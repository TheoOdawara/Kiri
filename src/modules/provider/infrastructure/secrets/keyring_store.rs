//! [`SecretStore`] backed by the OS keyring (macOS Keychain / Windows Credential Manager / Linux
//! Secret Service) via the `keyring` crate. One entry per provider id, holding the credential as JSON.

use keyring::{Entry, Error as KeyringError};

use crate::modules::provider::application::secret_store::SecretStore;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Credential;

/// Keyring service name namespacing every Kiri credential. The provider id is the per-entry username.
const SERVICE: &str = "dev.kiri.credentials";

pub struct KeyringSecretStore;

impl KeyringSecretStore {
    pub fn new() -> Self {
        Self
    }

    /// Probe whether an OS keyring is reachable, so the composition root chooses keyring vs the file
    /// fallback once at startup (avoiding split-brain — saving to one backend, reading from the other).
    /// A non-existent probe entry means the store works but is empty → available; only a
    /// storage-access/platform failure (no Secret Service on headless Linux, etc.) means unavailable.
    pub fn is_available() -> bool {
        match Entry::new(SERVICE, "__kiri_probe__") {
            Ok(entry) => !matches!(
                entry.get_password(),
                Err(KeyringError::NoStorageAccess(_) | KeyringError::PlatformFailure(_))
            ),
            Err(_) => false,
        }
    }

    fn entry(provider_id: &str) -> Result<Entry, AgentError> {
        Entry::new(SERVICE, provider_id)
            .map_err(|e| AgentError::Secret(format!("keyring entry: {e}")))
    }
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, provider_id: &str) -> Result<Option<Credential>, AgentError> {
        match Self::entry(provider_id)?.get_password() {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| AgentError::Secret(format!("decode credential: {e}"))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(AgentError::Secret(format!("keyring read: {e}"))),
        }
    }

    fn set(&self, provider_id: &str, credential: &Credential) -> Result<(), AgentError> {
        let json = serde_json::to_string(credential)
            .map_err(|e| AgentError::Secret(format!("encode credential: {e}")))?;
        Self::entry(provider_id)?
            .set_password(&json)
            .map_err(|e| AgentError::Secret(format!("keyring write: {e}")))
    }

    fn delete(&self, provider_id: &str) -> Result<(), AgentError> {
        match Self::entry(provider_id)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(AgentError::Secret(format!("keyring delete: {e}"))),
        }
    }
}
