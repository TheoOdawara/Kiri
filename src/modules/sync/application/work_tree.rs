use std::path::Path;

use crate::shared::kernel::error::AgentResult;

/// Port: every filesystem touch the sync use-case needs on its work-tree and the live config, behind one
/// capability so `SyncService` orchestrates only ports (like its `Git` and `memory_ndjson` siblings) and
/// holds no inline `tokio::fs` or `.exists()`. The adapter (`infrastructure::fs_work_tree::FsSyncWorkTree`)
/// owns the I/O; it refuses to write through a non-regular existing target, so a hostile-remote-
/// materialized symlink in the tree cannot redirect a write out of it.
#[async_trait::async_trait]
pub trait SyncWorkTree: Send + Sync {
    /// Create `dir` and any missing parents (the work-tree root).
    async fn ensure_dir(&self, dir: &Path) -> AgentResult<()>;

    /// Write `contents` to `path`, refusing an existing non-regular target first.
    async fn write(&self, path: &Path, contents: &str) -> AgentResult<()>;

    /// Write `contents` to `path` atomically (temp sibling then rename), refusing an existing non-regular
    /// target first — so a crash mid-write can never leave a truncated/corrupt trusted config.
    async fn write_atomic(&self, path: &Path, contents: &str) -> AgentResult<()>;

    /// Copy `from` to `to`, refusing an existing non-regular `to` first.
    async fn copy(&self, from: &Path, to: &Path) -> AgentResult<()>;

    /// Read `path`, mapping a not-found to `Ok(None)` so the caller can distinguish "absent" (a first-pull
    /// empty baseline / "no config in sync") from a genuine read error (which it surfaces).
    async fn read_to_string(&self, path: &Path) -> AgentResult<Option<String>>;

    /// Whether `path` exists (covers the `.git`-presence and config-present checks).
    async fn exists(&self, path: &Path) -> AgentResult<bool>;
}
