//! Credential-store adapter: a 0600 file.

pub mod file_store;

use std::path::PathBuf;

use crate::modules::provider::application::secret_store::SecretStore;
use file_store::FileSecretStore;

/// Pick the credential store: a 0600 file at `fallback_file`.
pub fn default_secret_store(fallback_file: PathBuf) -> Box<dyn SecretStore> {
    Box::new(FileSecretStore::new(fallback_file))
}
