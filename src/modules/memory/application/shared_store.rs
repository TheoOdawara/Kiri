use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// Use cases for shared memory across projects. Implemented by `SqliteSharedStore` (adapter over
/// `SqliteSharedMemory`).
#[async_trait]
pub trait SharedStore: Send + Sync {
    /// Save an entry (create or update).
    async fn save(&self, entry: MemoryEntry) -> Result<()>;

    /// Search entries by text query.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries by kind. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries by tag. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List entries for a specific project. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_project(&self, project_id: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Persist the embedding vector for an entry (for semantic recall). Default no-op so a store without
    /// embedding support — and the test doubles — need not implement it.
    async fn save_embedding(&self, _entry_id: &str, _model: &str, _vector: &[f32]) -> Result<()> {
        Ok(())
    }

    /// Entries embedded under `model`, paired with their vector, up to `limit`. Scoped to the active
    /// embedder's model so cross-model vectors are never ranked. Default empty so a non-embedding store
    /// transparently falls back to keyword recall.
    async fn embedded_candidates(
        &self,
        _model: &str,
        _limit: usize,
    ) -> Result<Vec<(MemoryEntry, Vec<f32>)>> {
        Ok(Vec::new())
    }

    /// Whether the store is available.
    fn is_available(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    struct InMemorySharedStore {
        entries: Mutex<Vec<MemoryEntry>>,
        available: bool,
    }

    impl InMemorySharedStore {
        fn new(available: bool) -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                available,
            }
        }
    }

    #[async_trait]
    impl SharedStore for InMemorySharedStore {
        async fn save(&self, entry: MemoryEntry) -> Result<()> {
            self.entries.lock().unwrap().push(entry);
            Ok(())
        }

        async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|e| e.matches_query(query))
                .take(limit)
                .cloned()
                .collect())
        }

        async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>> {
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|e| e.kind == kind)
                .take(limit)
                .cloned()
                .collect())
        }

        async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|e| e.tags.contains(tag))
                .take(limit)
                .cloned()
                .collect())
        }

        async fn list_by_project(
            &self,
            project_id: &str,
            limit: usize,
        ) -> Result<Vec<MemoryEntry>> {
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|e| e.project_id.as_deref() == Some(project_id))
                .take(limit)
                .cloned()
                .collect())
        }

        fn is_available(&self) -> bool {
            self.available
        }
    }

    #[tokio::test]
    async fn shared_store_save_and_search() {
        let store = Arc::new(InMemorySharedStore::new(true));
        let entry = MemoryEntry::new(
            MemoryKind::Heuristic,
            "Prefer explicit error types over anyhow::Error in library code".into(),
            ["rust", "api-design"]
                .into_iter()
                .map(String::from)
                .collect(),
            Some("proj-abc123".into()),
        );
        store.save(entry).await.unwrap();

        let results = store.search("explicit error", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn shared_store_list_by_project() {
        let store = Arc::new(InMemorySharedStore::new(true));
        store
            .save(MemoryEntry::new(
                MemoryKind::Pattern,
                "proj-a pattern".into(),
                HashSet::new(),
                Some("proj-a".into()),
            ))
            .await
            .unwrap();
        store
            .save(MemoryEntry::new(
                MemoryKind::Fact,
                "global fact".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();

        let proj_a = store.list_by_project("proj-a", 10).await.unwrap();
        assert_eq!(proj_a.len(), 1);
        assert_eq!(proj_a[0].project_id.as_deref(), Some("proj-a"));

        let global = store.list_by_project("", 10).await.unwrap();
        // Empty project_id should not match None
        assert_eq!(global.len(), 0);
    }

    #[tokio::test]
    async fn shared_store_availability() {
        let store = Arc::new(InMemorySharedStore::new(false));
        assert!(!store.is_available());
    }
}
