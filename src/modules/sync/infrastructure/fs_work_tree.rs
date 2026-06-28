use std::path::Path;

use tokio::fs;

use crate::modules::sync::application::work_tree::SyncWorkTree;
use crate::shared::infra::fs as shared_fs;
use crate::shared::kernel::error::{AgentError, AgentResult};

/// Byte ceiling on a work-tree text read (`config.toml` / `.gitignore`). The tree is materialized by
/// `git reset --hard` from an untrusted remote, so a hostile config copy must not OOM the read; a real
/// config is a few KiB, so this cap is ample. (The memory NDJSON is read by `memory_ndjson::import`,
/// which has its own streaming guard — never through here.)
const MAX_WORK_TREE_READ_BYTES: u64 = 8 * 1024 * 1024;

/// The `tokio::fs` adapter for the sync work-tree. Every write refuses an existing non-regular target
/// (a symlink, directory, device) before writing — the write-side completion of the Wave-1 read guard —
/// so a hostile remote that materializes a symlink in the work-tree (`.gitignore`/`config.toml`/
/// `memory.ndjson`) cannot redirect that write outside the tree.
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
        // Guard the exact temp path `write_atomic` will write to, then delegate the temp-then-rename to the
        // single shared crash-safe primitive (`shared/infra/fs`). The symlink refusal stays here at the
        // adapter so a hostile remote's committed symlink — at the target or at the temp — cannot redirect
        // the write out of the work-tree.
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
        // The work-tree is materialized by `git reset --hard` from an untrusted remote, so guard the read
        // the same way `memory_ndjson::import` does: `symlink_metadata` (does NOT follow the link) rejects
        // a symlink or any non-regular file (a committed `-> /dev/zero` → OOM, `-> <fifo>` → blocking read
        // with no timeout) before it is opened, and a byte cap rejects an oversized config. A missing file
        // is `None` (no config in sync), preserving the former presence-check + read collapse (SEC-04).
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
        // ERR-02: `Path::exists()` collapses every IO error (permission denied, a stat failure) to
        // `false`, masking a real error behind a misleading "sync not initialized" / "no config".
        // `try_exists` reports NotFound as `Ok(false)` but propagates a genuine error.
        path.try_exists().map_err(sync_err)
    }
}

fn sync_err(e: std::io::Error) -> AgentError {
    AgentError::Sync(e.to_string())
}

/// Refuse to write through an existing target that is not a regular file (a symlink, directory, device).
/// `symlink_metadata` does not follow the link, so a symlink reports `is_file() == false` and is refused;
/// a regular file is allowed (overwrite), and an absent target is allowed (create).
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

    // ERR-02: a genuine stat failure must propagate, not collapse to `false`. A symlink loop makes
    // `try_exists` fail with ELOOP — a real error, distinct from NotFound.
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

    // SEC-04: the untrusted work-tree read must refuse a symlink (the `-> /dev/zero` / `-> <fifo>`
    // vectors) before following it — mirroring the import and write guards.
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

    // SEC-04: an oversized work-tree file is rejected by the up-front stat, before it is read. A sparse
    // `set_len` reports the large length without allocating it.
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
