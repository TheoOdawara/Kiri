use crate::modules::memory::application::memory_store::MemoryStore;
use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::application::shared_store::SharedStore;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::shared::kernel::error::AgentResult;

/// Application-level adapter exposing shared memory as the `MemoryStore` use-case surface plus the
/// `SharedStore` extension, delegating to the SQLite-backed `SqliteSharedMemory`. `available` records
/// whether `init` succeeded.
pub struct SqliteSharedStore {
    inner: SqliteSharedMemory,
    available: bool,
}

impl SqliteSharedStore {
    pub fn new(inner: SqliteSharedMemory, available: bool) -> Self {
        Self { inner, available }
    }
}

#[async_trait::async_trait]
impl MemoryStore for SqliteSharedStore {
    async fn save(&self, entry: MemoryEntry) -> AgentResult<()> {
        self.inner.save(&entry).await
    }

    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        self.inner.search(query, limit).await
    }

    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        self.inner.list_by_kind(kind, limit).await
    }

    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        self.inner.list_by_tag(tag, limit).await
    }

    async fn save_embedding(&self, entry_id: &str, model: &str, vector: &[f32]) -> AgentResult<()> {
        self.inner.save_embedding(entry_id, model, vector).await
    }

    async fn embedded_candidates(
        &self,
        model: &str,
        limit: usize,
    ) -> AgentResult<Vec<(MemoryEntry, Vec<f32>)>> {
        self.inner.embedded_candidates(model, limit).await
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

#[async_trait::async_trait]
impl SharedStore for SqliteSharedStore {
    async fn list_by_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<MemoryEntry>> {
        self.inner.list_by_project(project_id, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn delegates_to_sqlite_and_reports_availability() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("memory").join("shared.db");
        let inner = SqliteSharedMemory::new(db).unwrap();
        let available = inner.init().await.is_ok();
        let store = SqliteSharedStore::new(inner, available);
        assert!(store.is_available());

        store
            .save(MemoryEntry::new(
                MemoryKind::Decision,
                "Use SQLite for shared memory".into(),
                ["architecture"].into_iter().map(String::from).collect(),
                Some("proj-x".into()),
            ))
            .await
            .unwrap();

        assert_eq!(store.search("SQLite", 10).await.unwrap().len(), 1);
        assert_eq!(store.list_by_project("proj-x", 10).await.unwrap().len(), 1);
    }
}
