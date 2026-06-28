use crate::modules::memory::application::memory_store::MemoryStore;
use crate::modules::memory::domain::entry::MemoryEntry;
use crate::shared::kernel::error::AgentResult;

/// Use cases for shared memory across projects. Extends the base `MemoryStore` (save/search/embeddings)
/// with the cross-project `list_by_project`. Implemented by `SqliteSharedStore` (adapter over
/// `SqliteSharedMemory`).
#[async_trait::async_trait]
pub trait SharedStore: MemoryStore {
    /// List entries for a specific project. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<MemoryEntry>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
    use crate::modules::memory::infrastructure::test_support::InMemoryStore;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[tokio::test]
    async fn shared_store_save_and_search() {
        let store = Arc::new(InMemoryStore::new(true));
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
        let store = Arc::new(InMemoryStore::new(true));
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
        let store = Arc::new(InMemoryStore::new(false));
        assert!(!store.is_available());
    }
}
