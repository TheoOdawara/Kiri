use std::path::Path;

use crate::shared::kernel::error::AgentResult;

#[derive(Debug)]
pub struct MergeReport {
    pub merged: usize,
    /// Entries already present at an equal or newer timestamp.
    pub skipped: usize,
}

#[async_trait::async_trait]
pub trait MemoryExchange: Send + Sync {
    async fn export(&self, path: &Path) -> AgentResult<usize>;
    /// Last-write-wins. A missing file is an empty merge, not an error.
    async fn import(&self, path: &Path) -> AgentResult<MergeReport>;
}
