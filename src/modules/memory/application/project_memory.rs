use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// Persistence port for project-specific memory.
/// Implemented by `FileProjectMemory` (Markdown files under `.kiri/memory/`).
/// This is the store's full CRUD+query contract; init/save/search/list are already used by the wiring
/// and the tools, while the rest (load/delete/count/list_by_*) are exercised by tests and reserved for
/// the future memory-management UI.
#[allow(dead_code)]
#[async_trait]
pub trait ProjectMemory: Send + Sync {
    /// Initialize the storage (create directories, index, etc.).
    async fn init(&self) -> Result<()>;

    /// Save an entry (create or update by ID).
    async fn save(&self, entry: &MemoryEntry) -> Result<()>;

    /// Load an entry by ID.
    async fn load(&self, id: &str) -> Result<Option<MemoryEntry>>;

    /// Delete an entry by ID.
    async fn delete(&self, id: &str) -> Result<bool>;

    /// Search entries by text query (content, tags, kind).
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List all entries (with optional pagination).
    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries by kind.
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries by tag.
    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Count the total number of entries.
    async fn count(&self) -> Result<usize>;
}
