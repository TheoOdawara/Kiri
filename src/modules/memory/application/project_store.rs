use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentResult;

/// Use cases for project memory. Implemented by `FileProjectStore` (adapter over `FileProjectMemory`).
#[async_trait::async_trait]
pub trait ProjectStore: Send + Sync {
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
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    struct InMemoryProjectStore {
        entries: Mutex<Vec<MemoryEntry>>,
        available: bool,
    }

    impl InMemoryProjectStore {
        fn new(available: bool) -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                available,
            }
        }
    }

    #[async_trait::async_trait]
    impl ProjectStore for InMemoryProjectStore {
        async fn save(&self, entry: MemoryEntry) -> AgentResult<()> {
            self.entries.lock().unwrap().push(entry);
            Ok(())
        }

        async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|e| e.matches_query(query))
                .take(limit)
                .cloned()
                .collect())
        }

        async fn list_by_kind(
            &self,
            kind: MemoryKind,
            limit: usize,
        ) -> AgentResult<Vec<MemoryEntry>> {
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|e| e.kind == kind)
                .take(limit)
                .cloned()
                .collect())
        }

        async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|e| e.tags.contains(tag))
                .take(limit)
                .cloned()
                .collect())
        }

        fn is_available(&self) -> bool {
            self.available
        }
    }

    #[tokio::test]
    async fn project_store_save_and_search() {
        let store = Arc::new(InMemoryProjectStore::new(true));
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
    async fn project_store_list_by_kind() {
        let store = Arc::new(InMemoryProjectStore::new(true));
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
    async fn project_store_list_by_tag() {
        let store = Arc::new(InMemoryProjectStore::new(true));
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
    async fn project_store_availability() {
        let store = Arc::new(InMemoryProjectStore::new(false));
        assert!(!store.is_available());
    }
}
