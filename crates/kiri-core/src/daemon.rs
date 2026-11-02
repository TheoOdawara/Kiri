use std::fs::{self, File, TryLockError};
use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::env::{self, ConfigError};
use crate::protocol::transport::{self, Listener};
use crate::protocol::{Endpoint, framing};
use crate::provider::openai_compatible::OpenAiCompatible;
use crate::session::{Host, Sessions};

const OUTGOING_LINES: usize = 64;
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

    let provider = env::read().and_then(OpenAiCompatible::new);
    if let Err(cause) = &provider {
        tracing::warn!(%cause, "the configuration is invalid");
    }

    match serve(&endpoint, provider).await {
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

async fn serve(
    endpoint: &Endpoint,
    provider: Result<OpenAiCompatible, ConfigError>,
) -> io::Result<()> {
    let mut listener = Some(Listener::bind(endpoint)?);
    tracing::info!("daemon started");

    let (stop_listening, mut stop_requests) = mpsc::channel(1);
    let host = Arc::new(Host::new(provider, stop_listening));
    let mut connections = JoinSet::new();
    let no_client_timeout = tokio::time::sleep(NO_CLIENT_TIMEOUT);
    tokio::pin!(no_client_timeout);
    let mut had_a_client = false;

    // shortcut: the daemon exits when its last client leaves, #3 replaces this with reattach and an idle period.
    loop {
        tokio::select! {
            accepted = async {
                match listener.as_mut() {
                    Some(listener) => listener.accept().await,
                    None => std::future::pending().await,
                }
            } => match accepted {
                Ok(stream) => {
                    had_a_client = true;
                    connections.spawn(serve_connection(stream, Arc::clone(&host)));
                }
                Err(cause) => {
                    tracing::warn!(%cause, "could not accept a connection");
                    tokio::time::sleep(ACCEPT_RETRY).await;
                }
            },
            Some(stopped) = stop_requests.recv() => {
                listener = None;
                let _ = stopped.send(());
            }
            Some(_) = connections.join_next() => {
                if connections.is_empty() {
                    return Ok(());
                }
            }
            () = &mut no_client_timeout, if !had_a_client => return Ok(()),
        }
    }
}

async fn serve_connection<S>(stream: S, host: Arc<Host>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let (outgoing, lines) = mpsc::channel(OUTGOING_LINES);
    tokio::spawn(write_lines(writer, lines));
    let mut sessions = Sessions::new(host, outgoing);
    loop {
        let line = match framing::read_line(&mut reader).await {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(cause) => {
                tracing::warn!(%cause, "closing a connection");
                return;
            }
        };
        if sessions.handle(&line).await.is_err() {
            return;
        }
    }
}

async fn write_lines<W: AsyncWrite>(writer: W, mut lines: mpsc::Receiver<Vec<u8>>) {
    let mut writer = std::pin::pin!(writer);
    while let Some(line) = lines.recv().await {
        if let Err(cause) = writer.write_all(&line).await {
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
