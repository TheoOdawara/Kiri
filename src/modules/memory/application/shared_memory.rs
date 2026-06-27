use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// Persistence port for cross-project shared memory.
/// Implemented by `SqliteSharedMemory` (SQLite at `~/.kiri/memory/shared.db`).
/// Full CRUD+query contract; the methods not yet called by the agent loop are exercised by tests and
/// reserved for the future memory-management UI.
#[allow(dead_code)]
#[async_trait]
pub trait SharedMemory: Send + Sync {
    /// Initialize the storage (create DB, tables, indexes).
    async fn init(&self) -> Result<()>;

    /// Save an entry (create or update by ID).
    async fn save(&self, entry: &MemoryEntry) -> Result<()>;

    /// Load an entry by ID.
    async fn load(&self, id: &str) -> Result<Option<MemoryEntry>>;

    /// Delete an entry by ID.
    async fn delete(&self, id: &str) -> Result<bool>;

    /// Search entries by text query.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List all entries (with pagination).
    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries by kind.
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries by tag.
    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries for a specific project (by project_id).
    async fn list_by_project(&self, project_id: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Count the total number of entries.
    async fn count(&self) -> Result<usize>;

    /// Count entries for a project.
    async fn count_by_project(&self, project_id: &str) -> Result<usize>;
}
