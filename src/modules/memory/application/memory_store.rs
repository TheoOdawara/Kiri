use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentResult;

/// Base use-case port for a memory store, project or shared. The embedding methods carry a default, so a
/// store without embedding support falls back transparently to keyword recall.
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    /// Creates or updates by id.
    async fn save(&self, entry: MemoryEntry) -> AgentResult<()>;

    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Reserved for the memory-management UI.
    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Reserved for the memory-management UI.
    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Default no-op: a store without embedding support need not implement it.
    async fn save_embedding(
        &self,
        _entry_id: &str,
        _model: &str,
        _vector: &[f32],
    ) -> AgentResult<()> {
        Ok(())
    }

    /// Scoped to the active embedder's model, so cross-model vectors are never ranked against each other.
    async fn embedded_candidates(
        &self,
        _model: &str,
        _limit: usize,
    ) -> AgentResult<Vec<(MemoryEntry, Vec<f32>)>> {
        Ok(Vec::new())
    }

    /// Whether the store is initialized and reachable.
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
