use std::path::Path;

use crate::shared::kernel::error::AgentResult;

/// What an import merged versus skipped (an older or equal entry already present).
#[derive(Debug)]
pub struct MergeReport {
    pub merged: usize,
    pub skipped: usize,
}

/// Port: move the shared memory to/from the sync work-tree's portable NDJSON. A port (not the concrete
/// `memory_ndjson` adapter) so the sync use-case depends only inward — `SyncService` holds `&dyn
/// MemoryExchange`, and the composition root injects the NDJSON adapter bound to the shared store.
#[async_trait::async_trait]
pub trait MemoryExchange: Send + Sync {
    /// Export the shared memory into `path`, returning the entry count written.
    async fn export(&self, path: &Path) -> AgentResult<usize>;
    /// Import `path` into the shared memory (last-write-wins), returning what merged versus skipped. A
    /// missing file is an empty merge.
    async fn import(&self, path: &Path) -> AgentResult<MergeReport>;
}
