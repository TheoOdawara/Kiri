//! Persists project-layer active-capability approvals (ADR 0021's TOFU trust model): the user approves a
//! capability once, and it stays approved as long as its content hash matches — a changed file (a hostile
//! repo editing a hook after approval) reverts it to pending. Global-layer capabilities never consult this
//! store — `domain::gate::resolve` always approves them. Stored at `~/.kiri/extensions_trust.json`,
//! `0600`, mirroring the credentials file (`provider::infrastructure::secrets::FileSecretStore`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::shared::infra::config::ensure_private_dir;
use crate::shared::kernel::error::AgentError;

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustFile {
    /// Capability id -> the content hash last approved for it.
    approved: BTreeMap<String, String>,
}

/// The on-disk trust store: one approved content-hash per project-layer capability id.
// ponytail: no caller yet — the first active-capability type (hooks) lands in Fase 4 and records/consults
// approvals here before letting a project hook execute.
#[allow(dead_code)]
pub struct ExtensionsTrustStore {
    path: PathBuf,
}

#[allow(dead_code)]
impl ExtensionsTrustStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Whether `id` is currently approved for exactly `hash`. A prior approval under a different hash
    /// (the file changed since) reports `false` — the caller re-gates it as pending.
    pub fn is_approved(&self, id: &str, hash: &str) -> Result<bool, AgentError> {
        let file = self.read()?;
        Ok(file
            .approved
            .get(id)
            .is_some_and(|approved| approved == hash))
    }

    /// Record `id` as approved for `hash`, persisting immediately.
    pub fn approve(&self, id: &str, hash: &str) -> Result<(), AgentError> {
        let mut file = self.read()?;
        file.approved.insert(id.to_string(), hash.to_string());
        self.write(&file)
    }

    fn read(&self) -> Result<TrustFile, AgentError> {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) if raw.trim().is_empty() => Ok(TrustFile::default()),
            Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
                AgentError::extensions(format!("decode {}: {e}", self.path.display()))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TrustFile::default()),
            Err(e) => Err(AgentError::extensions(format!(
                "read {}: {e}",
                self.path.display()
            ))),
        }
    }

    fn write(&self, file: &TrustFile) -> Result<(), AgentError> {
        if let Some(parent) = self.path.parent() {
            ensure_private_dir(parent)
                .map_err(|e| AgentError::extensions(format!("create {}: {e}", parent.display())))?;
        }
        let json = serde_json::to_string_pretty(file)
            .map_err(|e| AgentError::extensions(format!("encode trust store: {e}")))?;
        write_owner_only(&self.path, json.as_bytes())
    }
}

/// Write `bytes` to `path` readable/writable by the owner only, mirroring `FileSecretStore`'s adapter
/// (crash-atomic on every platform; `0600` on Unix, the profile DACL on Windows — see its doc comment).
#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    crate::shared::infra::fs::write_atomic_owner_only(path, bytes)
        .map_err(|e| AgentError::extensions(format!("write {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    crate::shared::infra::fs::write_atomic_sync(path, bytes)
        .map_err(|e| AgentError::extensions(format!("write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn unapproved_capability_reports_false() {
        let dir = TempDir::new().unwrap();
        let store = ExtensionsTrustStore::new(dir.path().join("trust.json"));
        assert!(!store.is_approved("hook-a", "hash1").unwrap());
    }

    #[test]
    fn approve_then_is_approved_round_trips() {
        let dir = TempDir::new().unwrap();
        let store = ExtensionsTrustStore::new(dir.path().join("trust.json"));
        store.approve("hook-a", "hash1").unwrap();
        assert!(store.is_approved("hook-a", "hash1").unwrap());
    }

    #[test]
    fn a_changed_hash_reverts_to_unapproved() {
        let dir = TempDir::new().unwrap();
        let store = ExtensionsTrustStore::new(dir.path().join("trust.json"));
        store.approve("hook-a", "hash1").unwrap();
        // The file's content changed since approval — TOFU means the new content is pending again.
        assert!(!store.is_approved("hook-a", "hash2").unwrap());
    }

    #[test]
    fn approvals_for_other_capabilities_are_independent() {
        let dir = TempDir::new().unwrap();
        let store = ExtensionsTrustStore::new(dir.path().join("trust.json"));
        store.approve("hook-a", "hash1").unwrap();
        assert!(!store.is_approved("hook-b", "hash1").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn written_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("trust.json");
        let store = ExtensionsTrustStore::new(path.clone());
        store.approve("hook-a", "hash1").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "trust store file must be 0600, got {mode:o}");
    }

    #[test]
    fn write_creates_parent_dir() {
        let dir = TempDir::new().unwrap();
        let kiri_dir = dir.path().join("kiri");
        let store = ExtensionsTrustStore::new(kiri_dir.join("trust.json"));
        store.approve("hook-a", "hash1").unwrap();
        assert!(kiri_dir.is_dir());
    }
}
