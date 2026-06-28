use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::memory::domain::scope::Scope;
use crate::modules::memory::domain::similarity::rank_by_similarity;
use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::shared::kernel::error::AgentResult;

/// How many embedded entries to pull as the semantic candidate set before cosine-ranking. Bounded so the
/// brute-force ranking stays cheap without a vector index.
const SEMANTIC_CANDIDATES: usize = 200;

/// Upper bound on any single embed call — the query embedding for recall and the content embedding on
/// remember alike — so a slow/unreachable embeddings endpoint degrades to keyword recall (or skips the
/// write-path embedding) promptly instead of stalling.
const EMBED_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimum cosine similarity for a semantic hit to count. Below this a match is treated as noise (a
/// query that matches nothing semantically yields no semantic hits rather than the most-recent embedded
/// entries surfaced as if relevant). Cross-model mismatch is no longer the floor's burden: the candidate
/// fetch is scoped to the active embedder's `model()`, so foreign-model vectors are never ranked.
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

/// Unified memory capability for the AgentLoop. Combines access to project memory and shared memory.
#[async_trait::async_trait]
pub trait Memory: Send + Sync {
    /// Recall project memories relevant to the query.
    async fn recall_project(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Recall shared memories relevant to the query.
    async fn recall_shared(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>>;

    /// Recall hits for many queries in one pass, embedding ALL queries in a single batch round-trip
    /// instead of one per query. `result[i]` holds the hits for `queries[i]`. Used by the end-of-session
    /// distiller's dedup so N candidates cost one embed call, not N. The store is read once and not
    /// mutated here, so the caller must dedup the queries against each other separately.
    async fn recall_batch(
        &self,
        scope: Scope,
        queries: &[String],
        limit: usize,
    ) -> AgentResult<Vec<Vec<MemoryEntry>>>;

    /// Save a memory in the project scope.
    async fn remember_project(&self, entry: MemoryEntry) -> AgentResult<()>;

    /// Save a memory in the shared scope.
    async fn remember_shared(&self, entry: MemoryEntry) -> AgentResult<()>;

    /// Whether project memory is available.
    fn project_memory_available(&self) -> bool;

    /// Whether shared memory is available.
    fn shared_memory_available(&self) -> bool;
}

/// Layered `Memory` adapter composing a project store, a shared store, and an optional embedder. When an
/// `EmbeddingProvider` is present, recall is semantic (cosine over stored embeddings) with a transparent
/// keyword fallback; otherwise it is keyword-only.
pub struct LayeredMemory<P, S> {
    project_store: P,
    shared_store: S,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
}

impl<P, S> LayeredMemory<P, S> {
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

/// Batched sibling of `semantic_pick`: embed ALL `queries` in one call, then rank each against the same
/// candidate set. `result[i]` is the semantic hits for `queries[i]`. Returns per-query empties when there
/// are no candidates or the batch embed fails/misaligns, so the caller fills from keyword search.
async fn semantic_pick_batch(
    embedder: &dyn EmbeddingProvider,
    candidates: &[(MemoryEntry, Vec<f32>)],
    queries: &[String],
    limit: usize,
) -> Vec<Vec<MemoryEntry>> {
    if candidates.is_empty() || queries.is_empty() {
        return vec![Vec::new(); queries.len()];
    }
    let query_vecs = match tokio::time::timeout(EMBED_TIMEOUT, embedder.embed(queries)).await {
        Ok(Ok(vecs)) if vecs.len() == queries.len() => vecs,
        _ => return vec![Vec::new(); queries.len()],
    };
    let by_id: HashMap<&str, &MemoryEntry> =
        candidates.iter().map(|(e, _)| (e.id.as_str(), e)).collect();
    query_vecs
        .iter()
        .map(|query_vec| {
            let refs = candidates
                .iter()
                .map(|(entry, vector)| (entry.id.as_str(), vector.as_slice()));
            rank_by_similarity(query_vec, refs, limit, MIN_SIMILARITY)
                .iter()
                .filter_map(|id| by_id.get(id.as_str()).map(|e| (*e).clone()))
                .collect()
        })
        .collect()
}

#[async_trait::async_trait]
impl<P, S> Memory for LayeredMemory<P, S>
where
    P: crate::modules::memory::application::memory_store::MemoryStore + Send + Sync,
    S: crate::modules::memory::application::shared_store::SharedStore + Send + Sync,
{
    async fn recall_project(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let semantic = match &self.embedder {
            Some(embedder) => {
                // Best-effort: a candidate-fetch failure degrades this recall to keyword search.
                let candidates = self
                    .project_store
                    .embedded_candidates(embedder.model(), SEMANTIC_CANDIDATES)
                    .await
                    .unwrap_or_default();
                semantic_pick(embedder.as_ref(), candidates, query, limit).await
            }
            None => Vec::new(),
        };
        // Short-circuit: once semantic recall already fills `limit`, skip the keyword pass. The union
        // below is therefore best-effort — it runs only when the semantic set underfills the budget, so
        // a keyword-only match (or an entry with no embedding) is surfaced while there is spare room, but
        // it is not guaranteed a slot once semantic recall has saturated the limit.
        if semantic.len() >= limit {
            return Ok(semantic);
        }
        let keyword = self.project_store.search(query, limit).await?;
        Ok(merge_dedup(semantic, keyword, limit))
    }

    async fn recall_shared(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let semantic = match &self.embedder {
            Some(embedder) => {
                // Best-effort: a candidate-fetch failure degrades this recall to keyword search.
                let candidates = self
                    .shared_store
                    .embedded_candidates(embedder.model(), SEMANTIC_CANDIDATES)
                    .await
                    .unwrap_or_default();
                semantic_pick(embedder.as_ref(), candidates, query, limit).await
            }
            None => Vec::new(),
        };
        // See recall_project: best-effort union, skipped once semantic recall fills the limit.
        if semantic.len() >= limit {
            return Ok(semantic);
        }
        let keyword = self.shared_store.search(query, limit).await?;
        Ok(merge_dedup(semantic, keyword, limit))
    }

    async fn recall_batch(
        &self,
        scope: Scope,
        queries: &[String],
        limit: usize,
    ) -> AgentResult<Vec<Vec<MemoryEntry>>> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        // Semantic hits per query in one batched embed; then the keyword union per query (each `search`
        // is a cheap local read, mirroring `recall_*`'s best-effort union under the same limit).
        let mut semantic = match &self.embedder {
            Some(embedder) => {
                let model = embedder.model();
                let candidates = match scope {
                    Scope::Project => self
                        .project_store
                        .embedded_candidates(model, SEMANTIC_CANDIDATES),
                    Scope::Shared => self
                        .shared_store
                        .embedded_candidates(model, SEMANTIC_CANDIDATES),
                }
                .await
                .unwrap_or_default();
                semantic_pick_batch(embedder.as_ref(), &candidates, queries, limit).await
            }
            None => vec![Vec::new(); queries.len()],
        };
        let mut out = Vec::with_capacity(queries.len());
        for (i, query) in queries.iter().enumerate() {
            let sem = std::mem::take(&mut semantic[i]);
            if sem.len() >= limit {
                out.push(sem);
                continue;
            }
            let keyword = match scope {
                Scope::Project => self.project_store.search(query, limit),
                Scope::Shared => self.shared_store.search(query, limit),
            }
            .await?;
            out.push(merge_dedup(sem, keyword, limit));
        }
        Ok(out)
    }

    async fn remember_project(&self, entry: MemoryEntry) -> AgentResult<()> {
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

    async fn remember_shared(&self, entry: MemoryEntry) -> AgentResult<()> {
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
    use crate::modules::memory::infrastructure::test_support::InMemoryStore;
    use crate::shared::kernel::error::AgentError;
    use std::collections::HashSet;

    #[tokio::test]
    async fn memory_port_delegates_to_stores() {
        let project = InMemoryStore::new(true);
        let shared = InMemoryStore::new(true);
        let port = LayeredMemory::new(project, shared);

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
        let project = InMemoryStore::new(false);
        let shared = InMemoryStore::new(true);
        let port = LayeredMemory::new(project, shared);

        assert!(!port.project_memory_available());
        assert!(port.shared_memory_available());
    }

    /// Semantic recall over real file+SQLite stores with a deterministic fake embedder. The embedder maps
    /// a text to a 3-dim presence vector over ["alpha","beta","gamma"], so a query ranks the entry whose
    /// content shares its keyword first.
    mod semantic {
        use super::*;
        use crate::modules::memory::application::memory_store::MemoryStore;
        use crate::modules::memory::application::project_memory::ProjectMemory;
        use crate::modules::memory::application::shared_memory::SharedMemory;
        use crate::modules::memory::infrastructure::file_project_memory::FileProjectMemory;
        use crate::modules::memory::infrastructure::file_project_store::FileProjectStore;
        use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
        use crate::modules::memory::infrastructure::sqlite_shared_store::SqliteSharedStore;
        use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
        use tempfile::TempDir;

        /// Map a text to a 3-dim presence vector over ["alpha","beta","gamma"].
        fn presence_vector(text: &str) -> Vec<f32> {
            let t = text.to_lowercase();
            vec![
                t.contains("alpha") as i32 as f32,
                t.contains("beta") as i32 as f32,
                t.contains("gamma") as i32 as f32,
            ]
        }

        struct FakeEmbedder;

        #[async_trait::async_trait]
        impl EmbeddingProvider for FakeEmbedder {
            async fn embed(&self, texts: &[String]) -> AgentResult<Vec<Vec<f32>>> {
                Ok(texts.iter().map(|t| presence_vector(t)).collect())
            }
            fn model(&self) -> &str {
                "fake-embed"
            }
        }

        /// Same mapping as `FakeEmbedder` but a different model id, so a vector stored under another model
        /// is out of scope for this embedder's recall.
        struct OtherModelEmbedder;

        #[async_trait::async_trait]
        impl EmbeddingProvider for OtherModelEmbedder {
            async fn embed(&self, texts: &[String]) -> AgentResult<Vec<Vec<f32>>> {
                Ok(texts.iter().map(|t| presence_vector(t)).collect())
            }
            fn model(&self) -> &str {
                "other-model"
            }
        }

        /// Counts how many times `embed` is invoked, to prove `recall_batch` issues a single call for N
        /// queries rather than one per query.
        struct CountingEmbedder {
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl EmbeddingProvider for CountingEmbedder {
            async fn embed(&self, texts: &[String]) -> AgentResult<Vec<Vec<f32>>> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(texts.iter().map(|t| presence_vector(t)).collect())
            }
            fn model(&self) -> &str {
                "fake-embed"
            }
        }

        struct FailEmbedder;

        #[async_trait::async_trait]
        impl EmbeddingProvider for FailEmbedder {
            async fn embed(&self, _texts: &[String]) -> AgentResult<Vec<Vec<f32>>> {
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
            let port = LayeredMemory::new(project_store(&dir).await, shared_store(&dir).await)
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
            let port = LayeredMemory::new(project_store(&dir).await, shared_store(&dir).await)
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
            let port = LayeredMemory::new(project_store(&dir).await, shared_store(&dir).await)
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
            let port = LayeredMemory::new(project_store(&dir).await, shared_store(&dir).await)
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

        #[tokio::test]
        async fn recall_batch_embeds_all_queries_in_one_call() {
            use std::sync::atomic::{AtomicUsize, Ordering};
            let dir = TempDir::new().unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let port = LayeredMemory::new(project_store(&dir).await, shared_store(&dir).await)
                .with_embedder(Arc::new(CountingEmbedder {
                    calls: calls.clone(),
                }));

            port.remember_shared(MemoryEntry::new(
                MemoryKind::Fact,
                "an alpha subject".into(),
                HashSet::new(),
                Some("p".into()),
            ))
            .await
            .unwrap();
            // The write embeds the content once; reset so the assertion isolates the recall_batch cost.
            calls.store(0, Ordering::SeqCst);

            let queries = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
            let results = port.recall_batch(Scope::Shared, &queries, 5).await.unwrap();
            assert_eq!(results.len(), 3, "one result list per query");
            assert!(
                results[0].iter().any(|e| e.content.contains("alpha")),
                "the alpha query must recall the alpha entry"
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "three queries must cost a single batched embed call, not three"
            );
        }

        #[tokio::test]
        async fn recall_ignores_other_model_vectors() {
            let dir = TempDir::new().unwrap();
            let shared = shared_store(&dir).await;

            // A body sharing no literal token with the query, embedded under model "a" with the exact
            // vector the "alpha" query produces. A cross-model rank would surface it on a perfect cosine;
            // scoping the candidate fetch to the active model must keep it out, degrading to keyword.
            let entry = MemoryEntry::new(
                MemoryKind::Fact,
                "the first greek letter".into(),
                HashSet::new(),
                Some("p".into()),
            );
            shared.save(entry.clone()).await.unwrap();
            shared
                .save_embedding(&entry.id, "a", &[1.0, 0.0, 0.0])
                .await
                .unwrap();

            let port = LayeredMemory::new(project_store(&dir).await, shared)
                .with_embedder(Arc::new(OtherModelEmbedder));

            let hits = port.recall_shared("alpha", 5).await.unwrap();
            assert!(
                hits.is_empty(),
                "a vector from another model must not be ranked: {hits:?}"
            );
        }
    }
}
