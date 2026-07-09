use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentResult;

/// Persistence port for project-specific memory (ADR 0010), implemented by `FileProjectMemory` over
/// Markdown files under `.kiri/memory/`.
#[async_trait::async_trait]
pub trait ProjectMemory: Send + Sync {
    async fn init(&self) -> AgentResult<()>;

    /// Creates or updates by id.
    async fn save(&self, entry: &MemoryEntry) -> AgentResult<()>;

    async fn load(&self, id: &str) -> AgentResult<Option<MemoryEntry>>;

    /// Matches against content, tags, and kind.
    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    async fn list(&self, offset: usize, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Reserved for the memory-management UI.
    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Reserved for the memory-management UI.
    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;
}
