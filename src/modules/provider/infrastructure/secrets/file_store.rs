//! The whole credential map is read-modify-written per call — fine for the handful of providers a user
//! configures, and the reason there is no caching layer here (ADR 0020).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::modules::provider::application::secret_store::SecretStore;
use crate::shared::infra::config::ensure_private_dir;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Credential;

pub struct FileSecretStore {
    path: PathBuf,
}

impl FileSecretStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_all(&self) -> Result<BTreeMap<String, Credential>, AgentError> {
        match fs::read_to_string(&self.path) {
            Ok(raw) if raw.trim().is_empty() => Ok(BTreeMap::new()),
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|e| AgentError::Secret(format!("decode {}: {e}", self.path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(AgentError::Secret(format!(
                "read {}: {e}",
                self.path.display()
            ))),
        }
    }

    fn write_all(&self, map: &BTreeMap<String, Credential>) -> Result<(), AgentError> {
        if let Some(parent) = self.path.parent() {
            // Never a plain `create_dir_all`: the dir holding `credentials.json` must be 0700.
            ensure_private_dir(parent)
                .map_err(|e| AgentError::Secret(format!("create {}: {e}", parent.display())))?;
        }
        let json = serde_json::to_string_pretty(map)
            .map_err(|e| AgentError::Secret(format!("encode credentials: {e}")))?;
        write_owner_only(&self.path, json.as_bytes())
    }
}

impl SecretStore for FileSecretStore {
    fn get(&self, provider_id: &str) -> Result<Option<Credential>, AgentError> {
        Ok(self.read_all()?.remove(provider_id))
    }

    fn set(&self, provider_id: &str, credential: &Credential) -> Result<(), AgentError> {
        let mut map = self.read_all()?;
        map.insert(provider_id.to_string(), credential.clone());
        self.write_all(&map)
    }

    fn delete(&self, provider_id: &str) -> Result<(), AgentError> {
        let mut map = self.read_all()?;
        if map.remove(provider_id).is_some() {
            self.write_all(&map)?;
        }
        Ok(())
    }
}

/// Both branches are crash-atomic: a partial write here would lose every stored key. On Windows std
/// exposes no ACL control, so the file inherits the user-profile DACL — the accepted equivalent of 0600.
#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    crate::shared::infra::fs::write_atomic_owner_only(path, bytes)
        .map_err(|e| AgentError::Secret(format!("write {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    crate::shared::infra::fs::write_atomic_sync(path, bytes)
        .map_err(|e| AgentError::Secret(format!("write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::provider::Secret;
    use tempfile::TempDir;

    #[test]
    fn set_get_delete_round_trip() {
        let dir = TempDir::new().unwrap();
        let store = FileSecretStore::new(dir.path().join("credentials.json"));

        assert!(store.get("nvidia").unwrap().is_none());

        let cred = Credential::ApiKey {
            key: Secret::new("sk-xyz"),
        };
        store.set("nvidia", &cred).unwrap();

        match store.get("nvidia").unwrap() {
            Some(Credential::ApiKey { key }) => assert_eq!(key.expose(), "sk-xyz"),
            other => panic!("expected api-key, got {other:?}"),
        }

        store.delete("nvidia").unwrap();
        assert!(store.get("nvidia").unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn written_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");
        let store = FileSecretStore::new(path.clone());
        store
            .set(
                "p",
                &Credential::ApiKey {
                    key: Secret::new("k"),
                },
            )
            .unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials file must be 0600, got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn write_is_atomic_preserving_prior_keys_and_leaving_no_temp() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");
        let store = FileSecretStore::new(path.clone());
        store
            .set(
                "a",
                &Credential::ApiKey {
                    key: Secret::new("ka"),
                },
            )
            .unwrap();
        store
            .set(
                "b",
                &Credential::ApiKey {
                    key: Secret::new("kb"),
                },
            )
            .unwrap();
        assert!(store.get("a").unwrap().is_some());
        assert!(store.get("b").unwrap().is_some());
        assert!(
            !dir.path().join(".credentials.json.kiri-tmp").exists(),
            "the atomic write must leave no temp sibling"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_all_creates_parent_dir_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        // Must not pre-exist, so `write_all` has to create it.
        let kiri_dir = dir.path().join("kiri");
        let store = FileSecretStore::new(kiri_dir.join("credentials.json"));
        store
            .set(
                "p",
                &Credential::ApiKey {
                    key: Secret::new("k"),
                },
            )
            .unwrap();
        let mode = fs::metadata(&kiri_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "credential dir must be 0700, got {mode:o}");
    }
}
