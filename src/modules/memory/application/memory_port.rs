use crate::modules::memory::domain::entry::MemoryEntry;
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// Unified memory port for the AgentLoop. Combines access to project memory and shared memory.
#[async_trait]
pub trait MemoryPort: Send + Sync {
    /// Recall project memories relevant to the query.
    async fn recall_project(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Recall shared memories relevant to the query.
    async fn recall_shared(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Save a memory in the project scope.
    async fn remember_project(&self, entry: MemoryEntry) -> Result<()>;

    /// Save a memory in the shared scope.
    async fn remember_shared(&self, entry: MemoryEntry) -> Result<()>;

    /// Whether project memory is available.
    fn project_memory_available(&self) -> bool;

    /// Whether shared memory is available.
    fn shared_memory_available(&self) -> bool;
}

/// Default implementation that delegates to separate stores.
pub struct MemoryPortImpl<P, S> {
    project_store: P,
    shared_store: S,
}

impl<P, S> MemoryPortImpl<P, S> {
    pub fn new(project_store: P, shared_store: S) -> Self {
        Self {
            project_store,
            shared_store,
        }
    }
}

#[async_trait]
impl<P, S> MemoryPort for MemoryPortImpl<P, S>
where
    P: crate::modules::memory::application::project_store::ProjectStore + Send + Sync,
    S: crate::modules::memory::application::shared_store::SharedStore + Send + Sync,
{
    async fn recall_project(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.project_store.search(query, limit).await
    }

    async fn recall_shared(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.shared_store.search(query, limit).await
    }

    async fn remember_project(&self, entry: MemoryEntry) -> Result<()> {
        self.project_store.save(entry).await
    }

    async fn remember_shared(&self, entry: MemoryEntry) -> Result<()> {
        self.shared_store.save(entry).await
    }

    fn project_memory_available(&self) -> bool {
        self.project_store.is_available()
    }

    fn shared_memory_available(&self) -> bool {
        self.shared_store.is_available()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
    use std::collections::HashSet;
    use std::sync::Mutex;

    struct MockProjectStore {
        entries: Mutex<Vec<MemoryEntry>>,
        available: bool,
    }

    impl MockProjectStore {
        fn new(available: bool) -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                available,
            }
        }
    }

    #[async_trait]
    impl crate::modules::memory::application::project_store::ProjectStore for MockProjectStore {
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

    struct MockSharedStore {
        entries: Mutex<Vec<MemoryEntry>>,
        available: bool,
    }

    impl MockSharedStore {
        fn new(available: bool) -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                available,
            }
        }
    }

    #[async_trait]
    impl crate::modules::memory::application::shared_store::SharedStore for MockSharedStore {
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
    async fn memory_port_delegates_to_stores() {
        let project = MockProjectStore::new(true);
        let shared = MockSharedStore::new(true);
        let port = MemoryPortImpl::new(project, shared);

        let entry = MemoryEntry::new(
            MemoryKind::Pattern,
            "test pattern".into(),
            HashSet::new(),
            None,
        );
        port.remember_project(entry.clone()).await.unwrap();
        port.remember_shared(entry).await.unwrap();

        let results = port.recall_project("pattern", 10).await.unwrap();
        assert_eq!(results.len(), 1);

        let results = port.recall_shared("pattern", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn memory_port_reports_availability() {
        let project = MockProjectStore::new(false);
        let shared = MockSharedStore::new(true);
        let port = MemoryPortImpl::new(project, shared);

        assert!(!port.project_memory_available());
        assert!(port.shared_memory_available());
    }
}
