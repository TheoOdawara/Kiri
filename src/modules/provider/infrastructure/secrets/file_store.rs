//! [`SecretStore`] backed by a 0600 JSON file under the kiri config dir. The fallback when no OS
//! keyring is reachable (headless Linux / CI). The whole map is read-modify-written per call — fine for
//! the handful of providers a user configures.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::modules::provider::application::secret_store::SecretStore;
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
            fs::create_dir_all(parent)
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

/// Write `bytes` to `path` readable/writable by the owner only. On Unix this is enforced as `0600`
/// (set at create and coerced on a pre-existing file). On Windows std exposes no ACL control, so the
/// file inherits the user-profile DACL (owner + SYSTEM/Administrators) — the accepted equivalent.
#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| AgentError::Secret(format!("open {}: {e}", path.display())))?;
    // Coerce perms in case the file pre-existed with a wider mode (mode() only applies at create).
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| AgentError::Secret(format!("chmod {}: {e}", path.display())))?;
    file.write_all(bytes)
        .map_err(|e| AgentError::Secret(format!("write {}: {e}", path.display())))?;
    file.flush()
        .map_err(|e| AgentError::Secret(format!("flush {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    fs::write(path, bytes).map_err(|e| AgentError::Secret(format!("write {}: {e}", path.display())))
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
}
