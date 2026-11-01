use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EndpointError {
    #[error("Kiri cannot find your home directory.")]
    NoHome,
    #[error(
        "The path {} is too long for a Unix socket. The limit on this system is {limit} bytes.",
        path.display()
    )]
    SocketPathTooLong { path: PathBuf, limit: usize },
    #[error("Kiri cannot read your Windows user: {0}")]
    NoOwner(std::io::Error),
}

#[derive(Debug)]
pub struct Endpoint {
    home: PathBuf,
    kiri_dir: PathBuf,
    address: PathBuf,
}

impl Endpoint {
    pub fn resolve() -> Result<Self, EndpointError> {
        Self::for_home(crate::env::home_directory())
    }

    fn for_home(home: Option<PathBuf>) -> Result<Self, EndpointError> {
        let home = home
            .filter(|home| !home.as_os_str().is_empty())
            .ok_or(EndpointError::NoHome)?;
        let kiri_dir = home.join(".kiri");
        let address = super::transport::address(&kiri_dir)?;
        Ok(Self {
            home,
            kiri_dir,
            address,
        })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn log_path(&self) -> PathBuf {
        self.kiri_dir.join("logs").join("daemon.jsonl")
    }

    pub(crate) fn kiri_dir(&self) -> &Path {
        &self.kiri_dir
    }

    pub(crate) fn lock_path(&self) -> PathBuf {
        self.kiri_dir.join("daemon.lock")
    }

    pub(crate) fn address(&self) -> &Path {
        &self.address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_or_empty_home_is_no_home() {
        for home in [None, Some(PathBuf::new())] {
            let error = Endpoint::for_home(home).unwrap_err();
            assert_eq!(error.to_string(), "Kiri cannot find your home directory.");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_path_must_leave_room_for_the_terminating_nul() {
        let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
        let suffix = "/.kiri/daemon.sock".len();
        let home_of_length = |length: usize| PathBuf::from(format!("/{}", "k".repeat(length - 1)));

        assert!(Endpoint::for_home(Some(home_of_length(limit - 1 - suffix))).is_ok());
        assert!(matches!(
            Endpoint::for_home(Some(home_of_length(limit - suffix))),
            Err(EndpointError::SocketPathTooLong { .. })
        ));
    }
}
