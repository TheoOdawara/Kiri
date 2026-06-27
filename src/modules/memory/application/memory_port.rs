use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::memory::domain::similarity::rank_by_similarity;
use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// How many embedded entries to pull as the semantic candidate set before cosine-ranking. Bounded so the
/// brute-force ranking stays cheap without a vector index.
const SEMANTIC_CANDIDATES: usize = 200;

/// Upper bound on the query-embedding call, so a slow/unreachable embeddings endpoint falls back to
/// keyword recall promptly instead of stalling.
const EMBED_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimum cosine similarity for a semantic hit to count. Below this a match is treated as noise (a
/// query that matches nothing semantically yields no semantic hits rather than the most-recent embedded
/// entries surfaced as if relevant). It also blunts cross-model vector mismatch: vectors produced by a
/// different embedding model rarely clear the floor against the current query, so recall degrades to
/// keyword instead of ranking on meaningless cosines.
const MIN_SIMILARITY: f32 = 0.15;

/// Merge `primary` (semantic hits) with `secondary` (keyword hits), deduplicated by id and capped at
/// `limit`. Semantic hits come first; keyword fills the remainder, which both covers entries that have
/// no embedding (so they are never unreachable) and adds keyword matches the semantic floor excluded.
fn merge_dedup(
    primary: Vec<MemoryEntry>,
    secondary: Vec<MemoryEntry>,
    limit: usize,
) -> Vec<MemoryEntry> {
    let mut seen: HashSet<String> = primary.iter().map(|entry| entry.id.clone()).collect();
    let mut out = primary;
    for entry in secondary {
        if out.len() >= limit {
            break;
        }
        if seen.insert(entry.id.clone()) {
            out.push(entry);
        }
    }
    out.truncate(limit);
    out
}

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

/// Default implementation that delegates to separate stores. When an `EmbeddingProvider` is present,
/// recall is semantic (cosine over stored embeddings) with a transparent keyword fallback; otherwise it
/// is keyword-only.
pub struct MemoryPortImpl<P, S> {
    project_store: P,
    shared_store: S,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
}

impl<P, S> MemoryPortImpl<P, S> {
    pub fn new(project_store: P, shared_store: S) -> Self {
        Self {
            project_store,
            shared_store,
            embedder: None,
        }
    }

    /// Attach an embeddings provider, enabling semantic recall and embed-on-remember.
    pub fn with_embedder(mut self, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        self.embedder = Some(embedder);
        self
    }
}

/// Embed a single query string, bounded by `EMBED_TIMEOUT`. Returns `None` on any failure or timeout so
/// the caller falls back to keyword recall rather than surfacing an error.
async fn embed_query(embedder: &dyn EmbeddingProvider, text: &str) -> Option<Vec<f32>> {
    match tokio::time::timeout(EMBED_TIMEOUT, embedder.embed(&[text.to_string()])).await {
        Ok(Ok(mut vectors)) => vectors.drain(..).next(),
        _ => None,
    }
}

/// Rank embedded candidates against `query` (cosine, above `MIN_SIMILARITY`) and hydrate the top
/// entries. Returns an empty vec when there are no candidates, the query embedding failed, or nothing
/// clears the floor — the caller then fills from keyword search.
async fn semantic_pick(
    embedder: &dyn EmbeddingProvider,
    candidates: Vec<(MemoryEntry, Vec<f32>)>,
    query: &str,
    limit: usize,
) -> Vec<MemoryEntry> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let Some(query_vec) = embed_query(embedder, query).await else {
        return Vec::new();
    };
    let refs = candidates
        .iter()
        .map(|(entry, vector)| (entry.id.as_str(), vector.as_slice()));
    let ranked = rank_by_similarity(&query_vec, refs, limit, MIN_SIMILARITY);
    let by_id: HashMap<&str, &MemoryEntry> =
        candidates.iter().map(|(e, _)| (e.id.as_str(), e)).collect();
    ranked
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).map(|e| (*e).clone()))
        .collect()
}

#[async_trait]
impl<P, S> MemoryPort for MemoryPortImpl<P, S>
where
    P: crate::modules::memory::application::project_store::ProjectStore + Send + Sync,
    S: crate::modules::memory::application::shared_store::SharedStore + Send + Sync,
{
    async fn recall_project(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let semantic = match &self.embedder {
            Some(embedder) => {
                // Best-effort: a candidate-fetch failure degrades this recall to keyword search.
                let candidates = self
                    .project_store
                    .embedded_candidates(SEMANTIC_CANDIDATES)
                    .await
                    .unwrap_or_default();
                semantic_pick(embedder.as_ref(), candidates, query, limit).await
            }
            None => Vec::new(),
        };
        if semantic.len() >= limit {
            return Ok(semantic);
        }
        // Union with keyword recall so a strong keyword match — or an entry that has no embedding — is
        // never shadowed by the semantic set, and the floor's rejects are backfilled.
        let keyword = self.project_store.search(query, limit).await?;
        Ok(merge_dedup(semantic, keyword, limit))
    }

    async fn recall_shared(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let semantic = match &self.embedder {
            Some(embedder) => {
                // Best-effort: a candidate-fetch failure degrades this recall to keyword search.
                let candidates = self
                    .shared_store
                    .embedded_candidates(SEMANTIC_CANDIDATES)
                    .await
                    .unwrap_or_default();
                semantic_pick(embedder.as_ref(), candidates, query, limit).await
            }
            None => Vec::new(),
        };
        if semantic.len() >= limit {
            return Ok(semantic);
        }
        let keyword = self.shared_store.search(query, limit).await?;
        Ok(merge_dedup(semantic, keyword, limit))
    }

    async fn remember_project(&self, entry: MemoryEntry) -> Result<()> {
        let id = entry.id.clone();
        let content = entry.content.clone();
        self.project_store.save(entry).await?;
        if let Some(embedder) = &self.embedder
            && let Some(vector) = embed_query(embedder.as_ref(), &content).await
        {
            // Best-effort: a failed embedding only disables semantic recall for this one entry; the
            // entry is already saved, so keyword recall still finds it.
            let _ = self
                .project_store
                .save_embedding(&id, embedder.model(), &vector)
                .await;
        }
        Ok(())
    }

    async fn remember_shared(&self, entry: MemoryEntry) -> Result<()> {
        let id = entry.id.clone();
        let content = entry.content.clone();
        self.shared_store.save(entry).await?;
        if let Some(embedder) = &self.embedder
            && let Some(vector) = embed_query(embedder.as_ref(), &content).await
        {
            // Best-effort: see remember_project — the entry is saved regardless of the embedding.
            let _ = self
                .shared_store
                .save_embedding(&id, embedder.model(), &vector)
                .await;
        }
        Ok(())
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

    /// Semantic recall over real file+SQLite stores with a deterministic fake embedder. The embedder maps
    /// a text to a 3-dim presence vector over ["alpha","beta","gamma"], so a query ranks the entry whose
    /// content shares its keyword first.
    mod semantic {
        use super::*;
        use crate::modules::memory::application::project_memory::ProjectMemory;
        use crate::modules::memory::application::shared_memory::SharedMemory;
        use crate::modules::memory::infrastructure::file_project_memory::FileProjectMemory;
        use crate::modules::memory::infrastructure::file_project_store::FileProjectStore;
        use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
        use crate::modules::memory::infrastructure::sqlite_shared_store::SqliteSharedStore;
        use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
        use tempfile::TempDir;

        struct FakeEmbedder;

        #[async_trait]
        impl EmbeddingProvider for FakeEmbedder {
            async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
                Ok(texts
                    .iter()
                    .map(|t| {
                        let t = t.to_lowercase();
                        vec![
                            t.contains("alpha") as i32 as f32,
                            t.contains("beta") as i32 as f32,
                            t.contains("gamma") as i32 as f32,
                        ]
                    })
                    .collect())
            }
            fn model(&self) -> &str {
                "fake-embed"
            }
        }

        struct FailEmbedder;

        #[async_trait]
        impl EmbeddingProvider for FailEmbedder {
            async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
                Err(AgentError::Provider("embed boom".into()))
            }
            fn model(&self) -> &str {
                "fail-embed"
            }
        }

        async fn shared_store(dir: &TempDir) -> SqliteSharedStore {
            let inner = SqliteSharedMemory::new(dir.path().join("shared.db")).unwrap();
            let ok = inner.init().await.is_ok();
            SqliteSharedStore::new(inner, ok)
        }

        async fn project_store(dir: &TempDir) -> FileProjectStore {
            let inner = FileProjectMemory::new(dir.path().join(".kiri").join("memory"));
            let ok = inner.init().await.is_ok();
            FileProjectStore::new(inner, ok)
        }

        #[tokio::test]
        async fn ranks_shared_recall_by_cosine_similarity() {
            let dir = TempDir::new().unwrap();
            let port = MemoryPortImpl::new(project_store(&dir).await, shared_store(&dir).await)
                .with_embedder(Arc::new(FakeEmbedder));

            port.remember_shared(MemoryEntry::new(
                MemoryKind::Fact,
                "an alpha subject".into(),
                HashSet::new(),
                Some("p".into()),
            ))
            .await
            .unwrap();
            port.remember_shared(MemoryEntry::new(
                MemoryKind::Fact,
                "a gamma subject".into(),
                HashSet::new(),
                Some("p".into()),
            ))
            .await
            .unwrap();

            // The query has no literal substring overlap handled by keyword search would not order these;
            // semantic ranking puts the alpha entry first.
            let hits = port.recall_shared("alpha", 1).await.unwrap();
            assert_eq!(hits.len(), 1);
            assert!(
                hits[0].content.contains("alpha"),
                "got: {}",
                hits[0].content
            );
        }

        #[tokio::test]
        async fn ranks_project_recall_by_cosine_similarity() {
            let dir = TempDir::new().unwrap();
            let port = MemoryPortImpl::new(project_store(&dir).await, shared_store(&dir).await)
                .with_embedder(Arc::new(FakeEmbedder));

            port.remember_project(MemoryEntry::new(
                MemoryKind::Pattern,
                "the beta way".into(),
                HashSet::new(),
                Some("p".into()),
            ))
            .await
            .unwrap();
            port.remember_project(MemoryEntry::new(
                MemoryKind::Pattern,
                "the gamma way".into(),
                HashSet::new(),
                Some("p".into()),
            ))
            .await
            .unwrap();

            let hits = port.recall_project("beta", 1).await.unwrap();
            assert_eq!(hits.len(), 1);
            assert!(hits[0].content.contains("beta"));
        }

        #[tokio::test]
        async fn an_unrelated_query_returns_nothing_not_recent_entries() {
            let dir = TempDir::new().unwrap();
            let port = MemoryPortImpl::new(project_store(&dir).await, shared_store(&dir).await)
                .with_embedder(Arc::new(FakeEmbedder));
            port.remember_shared(MemoryEntry::new(
                MemoryKind::Fact,
                "an alpha subject".into(),
                HashSet::new(),
                Some("p".into()),
            ))
            .await
            .unwrap();

            // "delta" is none of the embedder's keywords → an all-zero query vector → cosine 0 with the
            // stored entry (below the floor) → no semantic hit; "delta" is no keyword match either. The
            // floor must keep this empty rather than surfacing the most-recent embedded entry.
            let hits = port.recall_shared("delta", 5).await.unwrap();
            assert!(
                hits.is_empty(),
                "an unrelated query must not surface recent entries: {hits:?}"
            );
        }

        #[tokio::test]
        async fn falls_back_to_keyword_when_embedding_fails() {
            let dir = TempDir::new().unwrap();
            let port = MemoryPortImpl::new(project_store(&dir).await, shared_store(&dir).await)
                .with_embedder(Arc::new(FailEmbedder));

            port.remember_shared(MemoryEntry::new(
                MemoryKind::Fact,
                "unique-token here".into(),
                HashSet::new(),
                Some("p".into()),
            ))
            .await
            .unwrap();

            // The embedder errors on the query; recall must still find the entry by keyword.
            let hits = port.recall_shared("unique-token", 5).await.unwrap();
            assert_eq!(hits.len(), 1);
        }
    }
}
