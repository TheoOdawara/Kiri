use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentResult;

/// Persistence port for cross-project shared memory.
/// Implemented by `SqliteSharedMemory` (SQLite at `~/.kiri/memory/shared.db`).
/// Trimmed to the wired surface: `init`/`save`/`load`/`search` back the `SharedStore` adapter and `list`
/// backs the sync export/import and the boot digest; `list_by_*` and `count` are not on a runtime path
/// (reached only via the store delegation and the sync/store tests), so they carry targeted allows and
/// stay reserved for the future memory-management UI. The speculative `delete`/`count_by_project` methods
/// were removed (no caller at all) — restore from git history when the UI is built. See ADR 0010.
#[async_trait::async_trait]
pub trait SharedMemory: Send + Sync {
    /// Initialize the storage (create DB, tables, indexes).
    async fn init(&self) -> AgentResult<()>;

    /// Save an entry (create or update by ID).
    async fn save(&self, entry: &MemoryEntry) -> AgentResult<()>;

    /// Load an entry by ID.
    async fn load(&self, id: &str) -> AgentResult<Option<MemoryEntry>>;

    /// Search entries by text query.
    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List all entries (with pagination).
    async fn list(&self, offset: usize, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List entries by kind. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List entries by tag. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List entries for a specific project (by project_id). Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<MemoryEntry>>;

    /// Count the total number of entries. Exercised by the sync and store tests; reserved for the future
    /// memory-management UI (no runtime caller yet).
    #[allow(dead_code)]
    async fn count(&self) -> AgentResult<usize>;
}
