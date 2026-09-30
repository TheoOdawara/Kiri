pub mod file_store;

use std::path::PathBuf;

use crate::modules::provider::application::secret_store::SecretStore;
use file_store::FileSecretStore;

pub fn default_secret_store(fallback_file: PathBuf) -> Box<dyn SecretStore> {
    Box::new(FileSecretStore::new(fallback_file))
}
