use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// Casos de uso para memória de projeto.
/// Implementado por `FileProjectStore` (adapter sobre `FileProjectMemory`).
#[async_trait]
pub trait ProjectStore: Send + Sync {
    /// Salva uma entrada (cria ou atualiza).
    async fn save(&self, entry: MemoryEntry) -> Result<()>;

    /// Busca entradas por query textual.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista entradas por tipo. Parte da superfície do store consumida pela futura UI de gestão de
    /// memória; ainda não chamada pelo agent loop.
    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista entradas por tag. Reservada para a futura UI de gestão de memória.
    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Verifica se o store está disponível (inicializado, acessível).
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

    #[async_trait]
    impl ProjectStore for InMemoryProjectStore {
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
