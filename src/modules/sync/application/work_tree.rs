use std::path::Path;

use crate::shared::kernel::error::AgentResult;

/// Every write refuses an existing non-regular target, so a symlink materialized by a hostile remote
/// cannot redirect it out of the work-tree.
#[async_trait::async_trait]
pub trait SyncWorkTree: Send + Sync {
    async fn ensure_dir(&self, dir: &Path) -> AgentResult<()>;

    async fn write(&self, path: &Path, contents: &str) -> AgentResult<()>;

    /// Temp sibling then rename, so a crash mid-write cannot leave a truncated trusted config.
    async fn write_atomic(&self, path: &Path, contents: &str) -> AgentResult<()>;

    async fn copy(&self, from: &Path, to: &Path) -> AgentResult<()>;

    /// `Ok(None)` on not-found, so the caller can tell an absent baseline from a read error.
    async fn read_to_string(&self, path: &Path) -> AgentResult<Option<String>>;

    async fn exists(&self, path: &Path) -> AgentResult<bool>;
}
