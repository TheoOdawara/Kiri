use async_trait::async_trait;

use crate::modules::memory::application::project_memory::ProjectMemory;
use crate::modules::memory::application::project_store::ProjectStore;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::modules::memory::infrastructure::file_project_memory::FileProjectMemory;
use crate::shared::kernel::error::AgentError;

type Result<T> = std::result::Result<T, AgentError>;

/// Application-level adapter exposing project memory as the reduced `ProjectStore` use-case surface,
/// delegating to the file-backed `FileProjectMemory`. `available` records whether `init` succeeded, so
/// a storage failure degrades to an inert store (the harness keeps running) instead of aborting.
pub struct FileProjectStore {
    inner: FileProjectMemory,
    available: bool,
}

impl FileProjectStore {
    pub fn new(inner: FileProjectMemory, available: bool) -> Self {
        Self { inner, available }
    }
}

#[async_trait]
impl ProjectStore for FileProjectStore {
    async fn save(&self, entry: MemoryEntry) -> Result<()> {
        self.inner.save(&entry).await
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.inner.search(query, limit).await
    }

    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.inner.list_by_kind(kind, limit).await
    }

    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.inner.list_by_tag(tag, limit).await
    }

    async fn save_embedding(&self, entry_id: &str, model: &str, vector: &[f32]) -> Result<()> {
        self.inner.save_embedding(entry_id, model, vector).await
    }

    async fn embedded_candidates(&self, limit: usize) -> Result<Vec<(MemoryEntry, Vec<f32>)>> {
        self.inner.embedded_candidates(limit).await
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    async fn store(dir: &TempDir) -> FileProjectStore {
        let root = dir.path().join(".kiri").join("memory");
        let inner = FileProjectMemory::new(root);
        let available = inner.init().await.is_ok();
        FileProjectStore::new(inner, available)
    }

    #[tokio::test]
    async fn save_then_search_and_filter() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        assert!(store.is_available());

        store
            .save(MemoryEntry::new(
                MemoryKind::Pattern,
                "Prefer guard clauses over nested ifs".into(),
                ["rust", "style"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();
        store
            .save(MemoryEntry::new(
                MemoryKind::Fact,
                "edition 2024 stabilized in 1.85".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();

        let hits = store.search("guard", 10).await.unwrap();
        assert_eq!(hits.len(), 1);

        let patterns = store.list_by_kind(MemoryKind::Pattern, 10).await.unwrap();
        assert_eq!(patterns.len(), 1);

        let tagged = store.list_by_tag("rust", 10).await.unwrap();
        assert_eq!(tagged.len(), 1);
    }
}
