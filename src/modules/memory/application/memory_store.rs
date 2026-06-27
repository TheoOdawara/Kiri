use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentResult;

/// Base use-case port for a memory store (project or shared). Implemented by `FileProjectStore` and
/// `SqliteSharedStore`; `SharedStore` extends it with the cross-project `list_by_project`. The embedding
/// methods carry a default so a store without embedding support — and the test doubles — need not
/// implement them.
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    /// Save an entry (create or update).
    async fn save(&self, entry: MemoryEntry) -> AgentResult<()>;

    /// Search entries by text query.
    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List entries by kind. Part of the store surface the future memory-management UI will consume;
    /// not yet called by the agent loop.
    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// List entries by tag. Reserved for the future memory-management UI.
    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Persist the embedding vector for an entry (for semantic recall). Default no-op so a store without
    /// embedding support — and the test doubles — need not implement it.
    async fn save_embedding(
        &self,
        _entry_id: &str,
        _model: &str,
        _vector: &[f32],
    ) -> AgentResult<()> {
        Ok(())
    }

    /// Entries embedded under `model`, paired with their vector, up to `limit`. Scoped to the active
    /// embedder's model so cross-model vectors are never ranked. Default empty so a non-embedding store
    /// transparently falls back to keyword recall.
    async fn embedded_candidates(
        &self,
        _model: &str,
        _limit: usize,
    ) -> AgentResult<Vec<(MemoryEntry, Vec<f32>)>> {
        Ok(Vec::new())
    }

    /// Whether the store is available (initialized, reachable).
    fn is_available(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
    use crate::modules::memory::infrastructure::test_support::InMemoryStore;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[tokio::test]
    async fn memory_store_save_and_search() {
        let store = Arc::new(InMemoryStore::new(true));
        let entry = MemoryEntry::new(
            MemoryKind::Pattern,
            "Use Result<T, E> for fallible operations".into(),
            ["rust", "error-handling"]
                .into_iter()
                .map(String::from)
                .collect(),
            None,
        );
        store.save(entry).await.unwrap();

        let results = store.search("Result", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Result"));
    }

    #[tokio::test]
    async fn memory_store_list_by_kind() {
        let store = Arc::new(InMemoryStore::new(true));
        store
            .save(MemoryEntry::new(
                MemoryKind::Pattern,
                "pattern 1".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();
        store
            .save(MemoryEntry::new(
                MemoryKind::Fact,
                "fact 1".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();

        let patterns = store.list_by_kind(MemoryKind::Pattern, 10).await.unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].kind, MemoryKind::Pattern);
    }

    #[tokio::test]
    async fn memory_store_list_by_tag() {
        let store = Arc::new(InMemoryStore::new(true));
        store
            .save(MemoryEntry::new(
                MemoryKind::Pattern,
                "content".into(),
                ["rust", "async"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();
        store
            .save(MemoryEntry::new(
                MemoryKind::Fact,
                "content".into(),
                ["python"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();

        let rust_entries = store.list_by_tag("rust", 10).await.unwrap();
        assert_eq!(rust_entries.len(), 1);
    }

    #[tokio::test]
    async fn memory_store_availability() {
        let store = Arc::new(InMemoryStore::new(false));
        assert!(!store.is_available());
    }
}
