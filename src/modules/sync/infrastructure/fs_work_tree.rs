use std::path::Path;

use tokio::fs;

use crate::modules::sync::application::work_tree::SyncWorkTree;
use crate::shared::infra::fs as shared_fs;
use crate::shared::kernel::error::{AgentError, AgentResult};

/// The tree is materialized by `git reset --hard` from an untrusted remote, so a hostile config copy
/// must not OOM the read. Real configs are a few KiB. (`memory.ndjson` streams through its own guard.)
const MAX_WORK_TREE_READ_BYTES: u64 = 8 * 1024 * 1024;

pub struct FsSyncWorkTree;

#[async_trait::async_trait]
impl SyncWorkTree for FsSyncWorkTree {
    async fn ensure_dir(&self, dir: &Path) -> AgentResult<()> {
        fs::create_dir_all(dir).await.map_err(sync_err)
    }

    async fn write(&self, path: &Path, contents: &str) -> AgentResult<()> {
        refuse_irregular_target(path)?;
        fs::write(path, contents).await.map_err(sync_err)
    }

    async fn write_atomic(&self, path: &Path, contents: &str) -> AgentResult<()> {
        refuse_irregular_target(path)?;
        // The temp sibling is a second write target, so it needs the same symlink refusal.
        refuse_irregular_target(&shared_fs::temp_sibling(path))?;
        shared_fs::write_atomic(path, contents.as_bytes())
            .await
            .map_err(sync_err)
    }

    async fn copy(&self, from: &Path, to: &Path) -> AgentResult<()> {
        refuse_irregular_target(to)?;
        fs::copy(from, to).await.map(|_| ()).map_err(sync_err)
    }

    async fn read_to_string(&self, path: &Path) -> AgentResult<Option<String>> {
        // SEC-04: `symlink_metadata` does not follow the link, so a committed `-> /dev/zero` (OOM) or
        // `-> <fifo>` (blocking read) is rejected before the file is ever opened.
        let metadata = match fs::symlink_metadata(path).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(sync_err(e)),
        };
        if !metadata.file_type().is_file() {
            return Err(AgentError::Sync(format!(
                "refusing to read a non-regular path in the sync work-tree (symlink or special file): {}",
                path.display()
            )));
        }
        if metadata.len() > MAX_WORK_TREE_READ_BYTES {
            return Err(AgentError::Sync(format!(
                "sync work-tree file too large ({} bytes > {MAX_WORK_TREE_READ_BYTES} byte cap): {}",
                metadata.len(),
                path.display()
            )));
        }
        fs::read_to_string(path).await.map(Some).map_err(sync_err)
    }

    async fn exists(&self, path: &Path) -> AgentResult<bool> {
        // ERR-02: `Path::exists()` would collapse a permission error into a misleading "no config".
        path.try_exists().map_err(sync_err)
    }
}

fn sync_err(e: std::io::Error) -> AgentError {
    AgentError::Sync(e.to_string())
}

/// `symlink_metadata` does not follow the link, so a symlink reports `is_file() == false` here.
/// Overwriting a regular file and creating an absent one both stay allowed.
fn refuse_irregular_target(path: &Path) -> AgentResult<()> {
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

    // A symlink loop makes `try_exists` fail with ELOOP — a real error, distinct from NotFound.
    #[cfg(unix)]
    #[tokio::test]
    async fn fs_work_tree_exists_propagates_a_stat_error() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        let wt = FsSyncWorkTree;
        assert!(
            wt.exists(&a).await.is_err(),
            "a symlink-loop stat error must propagate, not collapse to false"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fs_work_tree_read_refuses_a_symlink() {
        let inside = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let victim = outside.path().join("secret");
        std::fs::write(&victim, "secret").unwrap();
        let link = inside.path().join("config.toml");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let wt = FsSyncWorkTree;
        let error = wt.read_to_string(&link).await.unwrap_err();
        assert!(
            matches!(&error, AgentError::Sync(m) if m.contains("non-regular")),
            "a symlinked work-tree config must be refused before reading: {error:?}"
        );
    }

    // A sparse `set_len` reports the large length without allocating it.
    #[tokio::test]
    async fn fs_work_tree_read_rejects_oversized_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_WORK_TREE_READ_BYTES + 1).unwrap();
        drop(file);

        let wt = FsSyncWorkTree;
        let error = wt.read_to_string(&path).await.unwrap_err();
        assert!(
            matches!(&error, AgentError::Sync(m) if m.contains("too large")),
            "an oversized work-tree file must be rejected by the byte cap: {error:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fs_work_tree_refuses_write_through_a_symlink() {
        let inside = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let victim = outside.path().join("victim.toml");
        std::fs::write(&victim, "original").unwrap();
        let link = inside.path().join("config.toml");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let wt = FsSyncWorkTree;
        assert!(wt.write(&link, "redirected").await.is_err());
        assert!(wt.write_atomic(&link, "redirected").await.is_err());
        assert!(
            wt.copy(&victim, &link).await.is_err(),
            "copy through a symlink target must also be refused"
        );
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "original");
    }
}
