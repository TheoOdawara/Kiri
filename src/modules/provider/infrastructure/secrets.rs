//! Credential-store adapters: the OS keyring (primary) and a 0600 file (fallback). The factory probes
//! the keyring once at startup and picks one backend, so reads and writes never disagree.

pub mod file_store;
pub mod keyring_store;

use std::path::PathBuf;

use crate::modules::provider::application::secret_store::SecretStore;
use file_store::FileSecretStore;
use keyring_store::KeyringSecretStore;

/// Pick the credential store: the OS keyring when reachable, else a 0600 file at `fallback_file`.
pub fn default_secret_store(fallback_file: PathBuf) -> Box<dyn SecretStore> {
    if KeyringSecretStore::is_available() {
        Box::new(KeyringSecretStore::new())
    } else {
        Box::new(FileSecretStore::new(fallback_file))
    }
}
