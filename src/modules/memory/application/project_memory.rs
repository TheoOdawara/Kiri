use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentResult;

/// Persistence port for project-specific memory.
/// Implemented by `FileProjectMemory` (Markdown files under `.kiri/memory/`).
/// Trimmed to the wired surface: `init`/`save`/`load`/`search`/`list` are used by the wiring and the
/// `MemoryStore` adapter; `list_by_*` are reserved for the future memory-management UI (reached via the
/// `MemoryStore` delegation, hence the targeted allows). The speculative `delete`/`count` methods were
/// removed (no runtime caller) — restore from git history when the UI is built. See ADR 0010.
#[async_trait::async_trait]
pub trait ProjectMemory: Send + Sync {
    /// Initialize the storage (create directories, index, etc.).
    async fn init(&self) -> AgentResult<()>;

    /// Save an entry (create or update by ID).
    async fn save(&self, entry: &MemoryEntry) -> AgentResult<()>;

    /// Load an entry by ID.
    async fn load(&self, id: &str) -> AgentResult<Option<MemoryEntry>>;

    /// Search entries by text query (content, tags, kind).
    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List all entries (with optional pagination).
    async fn list(&self, offset: usize, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List entries by kind. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List entries by tag. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;
}
