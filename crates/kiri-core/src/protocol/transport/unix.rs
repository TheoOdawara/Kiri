use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use crate::protocol::{Endpoint, EndpointError};

pub(crate) type ClientStream = UnixStream;

const SOCKET_PATH_LIMIT: usize = if cfg!(target_os = "macos") { 104 } else { 108 };

pub(crate) fn address(kiri_dir: &Path) -> Result<PathBuf, EndpointError> {
    let path = kiri_dir.join("daemon.sock");
    if path.as_os_str().as_bytes().len() >= SOCKET_PATH_LIMIT {
        return Err(EndpointError::SocketPathTooLong {
            path,
            limit: SOCKET_PATH_LIMIT,
        });
    }
    Ok(path)
}

pub(crate) async fn connect(endpoint: &Endpoint) -> io::Result<ClientStream> {
    UnixStream::connect(endpoint.address()).await
}

pub(crate) struct Listener {
    listener: UnixListener,
    path: PathBuf,
}

impl Listener {
    pub(crate) fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let path = endpoint.address().to_path_buf();
        match std::fs::remove_file(&path) {
            Err(cause) if cause.kind() != io::ErrorKind::NotFound => return Err(cause),
            _ => {}
        }
        let listener = UnixListener::bind(&path)?;
        Ok(Self { listener, path })
    }

    pub(crate) async fn accept(&mut self) -> io::Result<UnixStream> {
        let (stream, _) = self.listener.accept().await?;
        Ok(stream)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Err(cause) = std::fs::remove_file(&self.path) {
            tracing::warn!(%cause, "could not remove the daemon socket");
        }
    }
}
