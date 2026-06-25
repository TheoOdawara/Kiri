use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// Port for project-specific memory persistence.
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

/// Port for cross-project shared memory persistence.
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

/// Generate a deterministic project ID from the workspace path.
/// Uses blake3 to produce a short, stable hash.
pub fn project_id_from_path(path: &std::path::Path) -> String {
    use blake3::Hasher;
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path_str = canonical.to_string_lossy();
    let mut hasher = Hasher::new();
    hasher.update(path_str.as_bytes());
    let hash = hasher.finalize();
    // Use only the first 16 chars (64 bits) for readability.
    hash.to_hex().as_str()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_is_deterministic() {
        let path = std::path::Path::new("/tmp/test-project");
        let id1 = project_id_from_path(path);
        let id2 = project_id_from_path(path);
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 16);
    }

    #[test]
    fn different_paths_different_ids() {
        let id1 = project_id_from_path(std::path::Path::new("/tmp/proj-a"));
        let id2 = project_id_from_path(std::path::Path::new("/tmp/proj-b"));
        assert_ne!(id1, id2);
    }
}
