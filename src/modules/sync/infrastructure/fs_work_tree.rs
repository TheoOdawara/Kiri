use std::path::Path;

use async_trait::async_trait;
use tokio::fs;

use crate::modules::sync::application::work_tree::SyncWorkTree;
use crate::shared::infra::fs as shared_fs;
use crate::shared::kernel::error::AgentError;

/// The `tokio::fs` adapter for the sync work-tree. Every write refuses an existing non-regular target
/// (a symlink, directory, device) before writing — the write-side completion of the Wave-1 read guard —
/// so a hostile remote that materializes a symlink in the work-tree (`.gitignore`/`config.toml`/
/// `memory.ndjson`) cannot redirect that write outside the tree.
pub struct FsSyncWorkTree;

#[async_trait]
impl SyncWorkTree for FsSyncWorkTree {
    async fn ensure_dir(&self, dir: &Path) -> Result<(), AgentError> {
        fs::create_dir_all(dir).await.map_err(sync_err)
    }

    async fn write(&self, path: &Path, contents: &str) -> Result<(), AgentError> {
        refuse_irregular_target(path)?;
        fs::write(path, contents).await.map_err(sync_err)
    }

    async fn write_atomic(&self, path: &Path, contents: &str) -> Result<(), AgentError> {
        refuse_irregular_target(path)?;
        // Guard the exact temp path `write_atomic` will write to, then delegate the temp-then-rename to the
        // single shared crash-safe primitive (`shared/infra/fs`). The symlink refusal stays here at the
        // adapter so a hostile remote's committed symlink — at the target or at the temp — cannot redirect
        // the write out of the work-tree.
        refuse_irregular_target(&shared_fs::temp_sibling(path))?;
        shared_fs::write_atomic(path, contents.as_bytes()).await
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<(), AgentError> {
        refuse_irregular_target(to)?;
        fs::copy(from, to).await.map(|_| ()).map_err(sync_err)
    }

    async fn read_to_string(&self, path: &Path) -> Result<Option<String>, AgentError> {
        match fs::read_to_string(path).await {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(sync_err(e)),
        }
    }

    async fn exists(&self, path: &Path) -> Result<bool, AgentError> {
        Ok(path.exists())
    }
}

fn sync_err(e: std::io::Error) -> AgentError {
    AgentError::Sync(e.to_string())
}

/// Refuse to write through an existing target that is not a regular file (a symlink, directory, device).
/// `symlink_metadata` does not follow the link, so a symlink reports `is_file() == false` and is refused;
/// a regular file is allowed (overwrite), and an absent target is allowed (create).
fn refuse_irregular_target(path: &Path) -> Result<(), AgentError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(()),
        Ok(_) => Err(AgentError::Sync(format!(
            "refusing to write through a non-regular path in the sync work-tree: {}",
            path.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(sync_err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn fs_work_tree_write_atomic_replaces_target() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("config.toml");
        let wt = FsSyncWorkTree;
        wt.write_atomic(&target, "a").await.unwrap();
        wt.write_atomic(&target, "b").await.unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "b");
        // No temp sibling lingers after a successful atomic write.
        assert!(!dir.path().join(".config.toml.kiri-tmp").exists());
    }

    #[tokio::test]
    async fn fs_work_tree_read_to_string_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let wt = FsSyncWorkTree;
        assert!(
            wt.read_to_string(&dir.path().join("absent"))
                .await
                .unwrap()
                .is_none()
        );
        std::fs::write(dir.path().join("present"), "x").unwrap();
        assert_eq!(
            wt.read_to_string(&dir.path().join("present"))
                .await
                .unwrap(),
            Some("x".to_string())
        );
    }

    #[tokio::test]
    async fn fs_work_tree_exists_reports_presence() {
        let dir = TempDir::new().unwrap();
        let wt = FsSyncWorkTree;
        let p = dir.path().join("f");
        assert!(!wt.exists(&p).await.unwrap());
        std::fs::write(&p, "x").unwrap();
        assert!(wt.exists(&p).await.unwrap());
    }

    // Carry-forward hardening: a write through a symlink-typed target must be refused, so a hostile remote
    // that materializes a symlink in the work-tree cannot redirect the write outside the tree.
    #[cfg(unix)]
    #[tokio::test]
    async fn fs_work_tree_refuses_write_through_a_symlink() {
        let inside = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let victim = outside.path().join("victim.toml");
        std::fs::write(&victim, "original").unwrap();
        // A symlink inside the tree pointing out of it (the attack the guard blocks).
        let link = inside.path().join("config.toml");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let wt = FsSyncWorkTree;
        assert!(wt.write(&link, "redirected").await.is_err());
        assert!(wt.write_atomic(&link, "redirected").await.is_err());
        assert!(
            wt.copy(&victim, &link).await.is_err(),
            "copy through a symlink target must also be refused"
        );
        // The victim outside the tree is untouched.
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "original");
    }
}
