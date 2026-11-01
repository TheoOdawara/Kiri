use std::fs::{self, File, TryLockError};
use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::task::JoinSet;

use crate::protocol::transport::{self, Listener};
use crate::protocol::{Endpoint, framing, jsonrpc};

const NO_CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_ROTATION_BYTES: u64 = 10 * 1024 * 1024;
const ACCEPT_RETRY: Duration = Duration::from_millis(50);

pub async fn run() -> ExitCode {
    let endpoint = match Endpoint::resolve() {
        Ok(endpoint) => endpoint,
        Err(cause) => {
            eprintln!("{cause}");
            return ExitCode::FAILURE;
        }
    };
    let log_path = endpoint.log_path();
    if let Err(cause) = transport::secure_directory(endpoint.kiri_dir()) {
        start_logging(&log_path);
        tracing::error!(%cause, "could not secure the Kiri directory");
        eprintln!(
            "Could not secure {}: {cause}",
            endpoint.kiri_dir().display()
        );
        return ExitCode::FAILURE;
    }
    let _lock = match take_lock(&endpoint) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            eprintln!("A Kiri daemon is already running.");
            return ExitCode::SUCCESS;
        }
        Err(cause) => {
            start_logging(&log_path);
            tracing::error!(%cause, "could not take the daemon lock");
            eprintln!("Could not take the daemon lock: {cause}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(cause) = rotate_log(&log_path, LOG_ROTATION_BYTES) {
        eprintln!("Could not rotate the daemon log: {cause}");
    }
    start_logging(&log_path);

    match serve(&endpoint).await {
        Ok(()) => {
            tracing::info!("daemon exited");
            ExitCode::SUCCESS
        }
        Err(cause) => {
            tracing::error!(%cause, "could not bind the daemon endpoint");
            eprintln!("Could not bind the daemon endpoint: {cause}");
            ExitCode::FAILURE
        }
    }
}

fn take_lock(endpoint: &Endpoint) -> io::Result<Option<File>> {
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(endpoint.lock_path())?;
    transport::restrict_file(&lock)?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(cause)) => Err(cause),
    }
}

fn rotate_log(log_path: &Path, rotation_bytes: u64) -> io::Result<()> {
    match fs::metadata(log_path) {
        Ok(metadata) if metadata.len() >= rotation_bytes => {
            fs::rename(log_path, log_path.with_extension("jsonl.1"))
        }
        Err(cause) if cause.kind() != io::ErrorKind::NotFound => Err(cause),
        _ => Ok(()),
    }
}

fn start_logging(log_path: &Path) {
    let opened = log_path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| File::options().create(true).append(true).open(log_path))
        .and_then(|log| transport::restrict_file(&log).map(|()| log));
    match opened {
        Ok(log) => tracing_subscriber::fmt()
            .json()
            .with_writer(Mutex::new(log))
            .init(),
        Err(cause) => eprintln!("Could not open the daemon log: {cause}"),
    }
}

async fn serve(endpoint: &Endpoint) -> io::Result<()> {
    let mut listener = Listener::bind(endpoint)?;
    tracing::info!("daemon started");

    let mut connections = JoinSet::new();
    let no_client_timeout = tokio::time::sleep(NO_CLIENT_TIMEOUT);
    tokio::pin!(no_client_timeout);
    let mut had_a_client = false;

    // shortcut: the daemon exits when its last client leaves, #3 replaces this with reattach and an idle period.
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    had_a_client = true;
                    connections.spawn(serve_connection(stream));
                }
                Err(cause) => {
                    tracing::warn!(%cause, "could not accept a connection");
                    tokio::time::sleep(ACCEPT_RETRY).await;
                }
            },
            Some(_) = connections.join_next() => {
                if connections.is_empty() {
                    return Ok(());
                }
            }
            () = &mut no_client_timeout, if !had_a_client => return Ok(()),
        }
    }
}

async fn serve_connection<S: AsyncRead + AsyncWrite>(stream: S) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    loop {
        let line = match framing::read_line(&mut reader).await {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(cause) => {
                tracing::warn!(%cause, "closing a connection");
                return;
            }
        };
        if let Some(answer) = jsonrpc::answer(&line)
            && let Err(cause) = writer.write_all(&answer).await
        {
            tracing::warn!(%cause, "closing a connection");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_at_the_limit_rotates_and_replaces_the_previous_one() {
        let dir = std::env::temp_dir().join(format!("kiri-rotate-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("daemon.jsonl");
        let rotated = dir.join("daemon.jsonl.1");
        fs::write(&rotated, "previous").unwrap();

        fs::write(&log, "small").unwrap();
        rotate_log(&log, 6).unwrap();
        assert!(log.exists());

        fs::write(&log, "at max").unwrap();
        rotate_log(&log, 6).unwrap();
        assert!(!log.exists());
        assert_eq!(fs::read_to_string(&rotated).unwrap(), "at max");

        fs::remove_dir_all(&dir).unwrap();
    }
}
