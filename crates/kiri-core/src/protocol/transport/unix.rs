use std::fs::{self, DirBuilder, File, Permissions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use crate::protocol::{ConnectError, Endpoint, EndpointError};

pub(crate) type ClientStream = UnixStream;

const SOCKET_PATH_LIMIT: usize = if cfg!(target_os = "macos") { 104 } else { 108 };
const OWNER_ONLY_DIRECTORY: u32 = 0o700;
const OWNER_ONLY_FILE: u32 = 0o600;

pub(crate) fn secure_directory(path: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(OWNER_ONLY_DIRECTORY)
        .create(path)?;
    if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, Permissions::from_mode(OWNER_ONLY_DIRECTORY))?;
    }
    Ok(())
}

pub(crate) fn restrict_file(file: &File) -> io::Result<()> {
    file.set_permissions(Permissions::from_mode(OWNER_ONLY_FILE))
}

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

pub(crate) async fn connect(endpoint: &Endpoint) -> Result<ClientStream, ConnectError> {
    Ok(UnixStream::connect(endpoint.address()).await?)
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
        let bound = Self { listener, path };
        fs::set_permissions(&bound.path, Permissions::from_mode(OWNER_ONLY_FILE))?;
        Ok(bound)
    }

    pub(crate) async fn accept(&mut self) -> io::Result<UnixStream> {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let owner = unsafe { libc::geteuid() };
        loop {
            let (stream, _) = self.listener.accept().await?;
            match stream.peer_cred() {
                Ok(peer) if peer.uid() == owner => return Ok(stream),
                Ok(peer) => {
                    tracing::warn!(uid = peer.uid(), "refused a connection from another user");
                }
                Err(cause) => {
                    tracing::warn!(%cause, "refused a connection whose user cannot be read");
                }
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Err(cause) = std::fs::remove_file(&self.path) {
            tracing::warn!(%cause, "could not remove the daemon socket");
        }
    }
}
