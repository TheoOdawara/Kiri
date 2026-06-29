use crate::modules::memory::application::project_memory::ProjectMemory;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::infra::fs::write_atomic;
use crate::shared::kernel::error::{AgentError, AgentResult};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;

/// Largest entry body read into memory, mirroring `docs_library`'s `MAX_FILE_BYTES`. Caps a single read
/// so a symlinked `/dev/zero` or an over-sized committed `.md` cannot exhaust memory.
const MAX_ENTRY_BYTES: u64 = 256 * 1024;

/// Lexically join `rel` onto `root`, returning `None` if `rel` is absolute or contains a `..` component.
/// A corrupted or merged `index.json` could carry a path that escapes the memory dir. This is only the
/// lexical first stage (mirrors the sandbox's `join_checked`); `resolve_contained` adds the canonicalize
/// backstop that also defeats a symlink escape. Total — it never touches the filesystem and never panics.
fn contained_join(root: &Path, rel: &str) -> Option<PathBuf> {
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            // ParentDir escapes the root; RootDir/Prefix are absolute — reject all of them.
            _ => return None,
        }
    }
    Some(root.join(rel))
}

/// Resolve a stored index path to a real, symlink-resolved path asserted to stay inside the memory root,
/// returning `None` when it is absent or escapes. `contained_join` rejects the lexical `..`/absolute case;
/// canonicalizing both sides then closes the symlink hole — an `index.json` entry of all-`Normal`
/// components plus a hostile committed symlink to `~/.ssh/id_rsa` would otherwise be followed by the read.
/// Mirrors the sandbox's `resolve_existing` (lexical join → canonicalize → assert within root).
async fn resolve_contained(root: &Path, rel: &str) -> Option<PathBuf> {
    let real_root = fs::canonicalize(root).await.ok()?;
    resolve_within(&real_root, root, rel).await
}

/// The inner resolve against an already-canonicalized `real_root`, so a caller iterating many entries
/// (`search`) canonicalizes the root once for the whole loop instead of per entry.
async fn resolve_within(real_root: &Path, root: &Path, rel: &str) -> Option<PathBuf> {
    let candidate = contained_join(root, rel)?;
    let real = fs::canonicalize(&candidate).await.ok()?;
    real.starts_with(real_root).then_some(real)
}

/// Read at most `MAX_ENTRY_BYTES` of `path` as lossy UTF-8. The bounded read — not a post-hoc slice — is
/// what caps memory even for an endless source such as `/dev/zero`.
async fn read_capped(path: &Path) -> AgentResult<String> {
    let file = fs::File::open(path).await?;
    let mut buf = Vec::new();
    file.take(MAX_ENTRY_BYTES).read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Markdown-based project memory storage with TOML front-matter.
/// Structure:
///   .kiri/memory/
///   ├── index.json          # Index: id -> { path, kind, tags, updated_at }
///   ├── architecture.md
///   ├── patterns.md
///   └── decisions/
///       └── 001-example.md
pub struct FileProjectMemory {
    root: PathBuf,
    index: Arc<RwLock<ProjectIndex>>,
    initialized: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ProjectIndex {
    entries: HashMap<String, IndexEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct IndexEntry {
    path: String,
    kind: MemoryKind,
    tags: Vec<String>,
    updated_at: String,
}

/// A stored embedding in the sidecar (`embeddings.json`), keyed by entry id. Kept out of `index.json`
/// so the human-readable index stays small; the sidecar is a derived cache (re-derivable from content).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredEmbedding {
    model: String,
    vector: Vec<f32>,
}

impl FileProjectMemory {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            index: Arc::new(RwLock::new(ProjectIndex::default())),
            initialized: Arc::new(AtomicBool::new(false)),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    async fn load_index(&self) -> AgentResult<()> {
        let path = self.index_path();
        if path.exists() {
            let content = fs::read_to_string(&path).await?;
            let index: ProjectIndex = serde_json::from_str(&content).map_err(AgentError::memory)?;
            *self.index.write().await = index;
        }
        Ok(())
    }

    async fn save_index(&self) -> AgentResult<()> {
        let index = self.index.read().await.clone();
        let content = serde_json::to_string_pretty(&index).map_err(AgentError::memory)?;
        write_atomic(&self.index_path(), content.as_bytes()).await?;
        Ok(())
    }

    fn embeddings_path(&self) -> PathBuf {
        self.root.join("embeddings.json")
    }

    async fn load_embeddings(&self) -> AgentResult<HashMap<String, StoredEmbedding>> {
        let path = self.embeddings_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let content = fs::read_to_string(&path).await?;
        serde_json::from_str(&content).map_err(AgentError::memory)
    }

    /// Persist (or replace) the embedding vector for an entry in the sidecar cache.
    pub async fn save_embedding(
        &self,
        entry_id: &str,
        model: &str,
        vector: &[f32],
    ) -> AgentResult<()> {
        let mut sidecar = self.load_embeddings().await?;
        sidecar.insert(
            entry_id.to_string(),
            StoredEmbedding {
                model: model.to_string(),
                vector: vector.to_vec(),
            },
        );
        let content = serde_json::to_string(&sidecar).map_err(AgentError::memory)?;
        write_atomic(&self.embeddings_path(), content.as_bytes()).await?;
        Ok(())
    }

    /// The most recently updated entries embedded under `model`, paired with their vector. Reads file
    /// bodies only for the (bounded) candidates, ranked by the in-memory index's `updated_at`. Scoping
    /// to `model` keeps cross-model vectors out of the ranking when the active embedder changes.
    pub async fn embedded_candidates(
        &self,
        model: &str,
        limit: usize,
    ) -> AgentResult<Vec<(MemoryEntry, Vec<f32>)>> {
        let sidecar = self.load_embeddings().await?;
        if sidecar.is_empty() {
            return Ok(Vec::new());
        }
        // Snapshot (id, rel-path, updated_at) under one read lock, then release it before any file I/O.
        let mut candidates: Vec<(String, String, String)> = {
            let index = self.index.read().await;
            sidecar
                .iter()
                .filter(|(_, embedding)| embedding.model == model)
                .filter_map(|(id, _)| {
                    index
                        .entries
                        .get(id)
                        .map(|e| (id.clone(), e.path.clone(), e.updated_at.clone()))
                })
                .collect()
        };
        candidates.sort_by(|a, b| b.2.cmp(&a.2));
        candidates.truncate(limit);

        // PERF-02: canonicalize the root ONCE for the whole loop (mirroring `search`), instead of
        // re-canonicalizing it — and re-locking the index — per candidate via `self.load`.
        let Ok(real_root) = fs::canonicalize(&self.root).await else {
            return Ok(Vec::new());
        };

        let mut out = Vec::new();
        for (id, rel, _) in candidates {
            // Skip a missing/corrupt/escaping candidate rather than failing the whole semantic set,
            // matching search()'s per-file resilience (one bad entry must not disable semantic recall).
            let Some(path) = resolve_within(&real_root, &self.root, &rel).await else {
                continue;
            };
            if let Ok(content) = read_capped(&path).await
                && let Ok(entry) = parse_markdown_file(&content)
                && let Some(embedding) = sidecar.get(&id)
            {
                out.push((entry, embedding.vector.clone()));
            }
        }
        Ok(out)
    }

    fn entry_path(&self, kind: MemoryKind, id: &str) -> PathBuf {
        // Use the full id: a UUID v7 prefix is a millisecond timestamp, so two entries of the same kind
        // saved in the same millisecond share their leading chars — a truncated name would collide and
        // one would silently overwrite the other (data loss).
        let filename = format!("{}-{}.md", kind.as_wire(), id);
        match kind {
            MemoryKind::Decision => self.root.join("decisions").join(filename),
            _ => self.root.join(filename),
        }
    }

    fn ensure_dirs(&self) -> AgentResult<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(self.root.join("decisions"))?;
        Ok(())
    }
}

/// Parse a memory entry from its Markdown file body: TOML front-matter between leading `+++` fences,
/// or — with no front-matter — the whole body as a `Fact`. TOML (not YAML) removes a YAML parser from
/// the attack surface of attacker-influenceable memory files, reusing the `toml` crate the config layer
/// already depends on. A file without `+++` fences — including a legacy `---` YAML file from before the
/// switch — has no parseable front-matter and falls through to the `Fact` body case (graceful, never a
/// parse error). Pure: it reads neither `self` nor the path.
// ponytail: no on-disk migration of pre-existing `---` files — pre-launch, there are no released users,
// so legacy dev files simply re-distill via the Fact fallback. Upgrade path: write a one-time `---`→`+++`
// converter here if a TOML-format change ships after there is an installed base.
fn parse_markdown_file(content: &str) -> AgentResult<MemoryEntry> {
    let front_matter = content
        .strip_prefix("+++\n")
        .and_then(|after| after.find("\n+++\n").map(|end| &after[..end]));

    let entry = if let Some(fm) = front_matter {
        toml::from_str(fm).map_err(AgentError::memory)?
    } else {
        // Fallback for a file without `+++` front-matter: treat the whole body as a Fact.
        MemoryEntry::new(MemoryKind::Fact, content.to_string(), HashSet::new(), None)
    };

    Ok(entry)
}

/// Render a memory entry as a Markdown file: TOML front-matter between `+++` fences, then the content body.
fn render_markdown_file(entry: &MemoryEntry) -> AgentResult<String> {
    let front_matter = toml::to_string(entry).map_err(AgentError::memory)?;
    Ok(format!("+++\n{}+++\n\n{}", front_matter, entry.content))
}

#[async_trait::async_trait]
impl ProjectMemory for FileProjectMemory {
    async fn init(&self) -> AgentResult<()> {
        self.ensure_dirs()?;
        self.load_index().await?;
        self.initialized.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn save(&self, entry: &MemoryEntry) -> AgentResult<()> {
        let path = self.entry_path(entry.kind, &entry.id);
        let content = render_markdown_file(entry)?;

        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Write the body atomically, then update the index. The body-before-index ordering is the
        // recovery contract: a crash after the body but before the index leaves an orphan file that
        // `search` simply never reaches (it walks the index), never a truncated/half-written entry.
        write_atomic(&path, content.as_bytes()).await?;

        // Update the index. The path is built by joining `root`, so stripping it back cannot fail; the
        // fallback to the full path keeps this total without an `unwrap`.
        let rel = path.strip_prefix(&self.root).unwrap_or(path.as_path());
        let mut index = self.index.write().await;
        index.entries.insert(
            entry.id.clone(),
            IndexEntry {
                path: rel.to_string_lossy().to_string(),
                kind: entry.kind,
                tags: entry.tags.iter().cloned().collect(),
                updated_at: entry.updated_at.clone(),
            },
        );
        drop(index);
        self.save_index().await?;
        Ok(())
    }

    async fn load(&self, id: &str) -> AgentResult<Option<MemoryEntry>> {
        let rel = {
            let index = self.index.read().await;
            let Some(index_entry) = index.entries.get(id) else {
                return Ok(None);
            };
            index_entry.path.clone()
        };

        // Re-validate the stored path resolves to a real file under `root`; an escaping or symlinked path
        // (corrupt/merged index, or a hostile committed symlink) is treated as absent rather than read
        // from outside the memory dir.
        let Some(path) = resolve_contained(&self.root, &rel).await else {
            return Ok(None);
        };

        let content = read_capped(&path).await?;
        let entry = parse_markdown_file(&content)?;
        Ok(Some(entry))
    }

    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        // Snapshot the candidate paths under the lock, then release it before any file I/O so a long
        // read can never block a concurrent writer (matches `list`/`list_by_kind`).
        let rels: Vec<String> = {
            let index = self.index.read().await;
            index.entries.values().map(|e| e.path.clone()).collect()
        };

        // Canonicalize the root once for the whole loop; if the memory dir is unresolvable there is
        // nothing to read.
        let Ok(real_root) = fs::canonicalize(&self.root).await else {
            return Ok(Vec::new());
        };

        let mut results = Vec::new();
        for rel in rels {
            if results.len() >= limit {
                break;
            }
            // Skip an entry whose stored path escapes the memory root (corrupt/merged index or a hostile
            // symlink) rather than reading outside the dir; one bad entry must not blank out other matches.
            let Some(path) = resolve_within(&real_root, &self.root, &rel).await else {
                continue;
            };
            // Deliberately skip an unreadable entry rather than fail the whole search: one corrupt or
            // racing file must not blank out every other match.
            if let Ok(content) = read_capped(&path).await
                && let Ok(entry) = parse_markdown_file(&content)
                && entry.matches_query(query)
            {
                results.push(entry);
            }
        }
        Ok(results)
    }

    async fn list(&self, offset: usize, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let index = self.index.read().await;
        let ids: Vec<String> = index.entries.keys().cloned().collect();
        drop(index);

        let mut results = Vec::new();
        for id in ids.into_iter().skip(offset).take(limit) {
            if let Some(entry) = self.load(&id).await? {
                results.push(entry);
            }
        }
        Ok(results)
    }

    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let index = self.index.read().await;
        let ids: Vec<String> = index
            .entries
            .iter()
            .filter(|(_, e)| e.kind == kind)
            .map(|(id, _)| id.clone())
            .collect();
        drop(index);

        let mut results = Vec::new();
        for id in ids.into_iter().take(limit) {
            if let Some(entry) = self.load(&id).await? {
                results.push(entry);
            }
        }
        Ok(results)
    }

    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let index = self.index.read().await;
        let ids: Vec<String> = index
            .entries
            .iter()
            .filter(|(_, e)| e.tags.iter().any(|t| t == tag))
            .map(|(id, _)| id.clone())
            .collect();
        drop(index);

        let mut results = Vec::new();
        for id in ids.into_iter().take(limit) {
            if let Some(entry) = self.load(&id).await? {
                results.push(entry);
            }
        }
        Ok(results)
    }
}

#[async_trait::async_trait]
impl crate::modules::memory::application::memory_store::MemoryStore for FileProjectMemory {
    async fn save(&self, entry: MemoryEntry) -> AgentResult<()> {
        ProjectMemory::save(self, &entry).await
    }

    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        ProjectMemory::search(self, query, limit).await
    }

    #[allow(dead_code)]
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        ProjectMemory::list_by_kind(self, kind, limit).await
    }

    #[allow(dead_code)]
    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        ProjectMemory::list_by_tag(self, tag, limit).await
    }

    async fn save_embedding(&self, entry_id: &str, model: &str, vector: &[f32]) -> AgentResult<()> {
        FileProjectMemory::save_embedding(self, entry_id, model, vector).await
    }

    async fn embedded_candidates(
        &self,
        model: &str,
        limit: usize,
    ) -> AgentResult<Vec<(MemoryEntry, Vec<f32>)>> {
        FileProjectMemory::embedded_candidates(self, model, limit).await
    }

    fn is_available(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
    use std::collections::HashSet;
    use tempfile::TempDir;

    #[tokio::test]
    async fn file_project_memory_save_and_load() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        let entry = MemoryEntry::new(
            MemoryKind::Pattern,
            "Always use ?. for optional chaining".into(),
            ["rust", "style"].into_iter().map(String::from).collect(),
            None,
        );

        memory.save(&entry).await.unwrap();

        let loaded = memory.load(&entry.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, entry.id);
        assert_eq!(loaded.kind, MemoryKind::Pattern);
        assert_eq!(loaded.content, entry.content);
        assert!(loaded.tags.contains("rust"));
        assert!(loaded.tags.contains("style"));
    }

    #[test]
    fn front_matter_round_trips_as_toml() {
        // BUILD-05: front-matter is TOML between `+++` fences; render→parse must round-trip the entry.
        let entry = MemoryEntry::new(
            MemoryKind::Pattern,
            "Prefer guard clauses".into(),
            ["rust", "style"].into_iter().map(String::from).collect(),
            None,
        );
        let rendered = render_markdown_file(&entry).unwrap();
        assert!(rendered.starts_with("+++\n"));
        let parsed = parse_markdown_file(&rendered).unwrap();
        assert_eq!(parsed.id, entry.id);
        assert_eq!(parsed.kind, MemoryKind::Pattern);
        assert_eq!(parsed.content, entry.content);
        assert!(parsed.tags.contains("rust"));
        assert!(parsed.tags.contains("style"));
    }

    #[test]
    fn legacy_yaml_front_matter_falls_back_to_fact() {
        // A pre-switch `---` YAML file has no `+++` front-matter, so it must parse as a Fact body rather
        // than erroring — the graceful, no-migration path for pre-launch dev files.
        let legacy = "---\nid: x\nkind: pattern\n---\n\nold body";
        let parsed = parse_markdown_file(legacy).unwrap();
        assert_eq!(parsed.kind, MemoryKind::Fact);
        assert!(parsed.content.contains("old body"));
    }

    #[tokio::test]
    async fn file_project_memory_search() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        memory
            .save(&MemoryEntry::new(
                MemoryKind::Pattern,
                "Use Option<T> for nullable values".into(),
                ["rust"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();
        memory
            .save(&MemoryEntry::new(
                MemoryKind::Fact,
                "Python has None".into(),
                ["python"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();

        let results = memory.search("Option", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Option"));
    }

    #[tokio::test]
    async fn file_project_memory_list_by_kind() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        memory
            .save(&MemoryEntry::new(
                MemoryKind::Pattern,
                "pattern 1".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();
        memory
            .save(&MemoryEntry::new(
                MemoryKind::Fact,
                "fact 1".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();

        let patterns = memory.list_by_kind(MemoryKind::Pattern, 10).await.unwrap();
        assert_eq!(patterns.len(), 1);
    }

    #[tokio::test]
    async fn file_project_memory_list_by_tag() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        memory
            .save(&MemoryEntry::new(
                MemoryKind::Fact,
                "tagged rust".into(),
                ["rust"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();
        memory
            .save(&MemoryEntry::new(
                MemoryKind::Fact,
                "tagged python".into(),
                ["python"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();

        let rust = memory.list_by_tag("rust", 10).await.unwrap();
        assert_eq!(rust.len(), 1);
        assert!(rust[0].content.contains("rust"));
    }

    #[tokio::test]
    async fn file_project_memory_persists_index() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");

        // First instance saves.
        {
            let memory = FileProjectMemory::new(root.clone());
            memory.init().await.unwrap();
            let entry = MemoryEntry::new(
                MemoryKind::Heuristic,
                "persist test".into(),
                HashSet::new(),
                None,
            );
            memory.save(&entry).await.unwrap();
        }

        // Second instance loads from the index.
        {
            let memory = FileProjectMemory::new(root.clone());
            memory.init().await.unwrap();
            let entries = memory.list(0, 100).await.unwrap();
            assert_eq!(entries.len(), 1);
        }
    }

    #[tokio::test]
    async fn embedded_candidates_filters_by_model() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root);
        memory.init().await.unwrap();

        let a = MemoryEntry::new(MemoryKind::Fact, "content a".into(), HashSet::new(), None);
        let b = MemoryEntry::new(MemoryKind::Fact, "content b".into(), HashSet::new(), None);
        memory.save(&a).await.unwrap();
        memory.save(&b).await.unwrap();
        memory
            .save_embedding(&a.id, "model-a", &[1.0, 0.0])
            .await
            .unwrap();
        memory
            .save_embedding(&b.id, "model-b", &[0.0, 1.0])
            .await
            .unwrap();

        let candidates = memory.embedded_candidates("model-a", 10).await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0.id, a.id);
    }

    #[tokio::test]
    async fn embedded_candidates_skips_escaping_path() {
        // The PERF-02 refactor must keep search()'s containment resilience: an escaping index path is
        // skipped, never read from outside the memory root, even when it carries an embedding.
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        seed_escaping_index(&dir, &root, "leaked secret").await;

        let memory = FileProjectMemory::new(root);
        memory.init().await.unwrap();
        memory.save_embedding("evil", "m", &[1.0]).await.unwrap();

        let candidates = memory.embedded_candidates("m", 10).await.unwrap();
        assert!(
            candidates.is_empty(),
            "an escaping candidate path must be skipped, not read"
        );
    }

    /// Hand-write an index whose stored path escapes the memory root, with a real file at the escaped
    /// location, so the only thing keeping it out of reach is the containment check.
    async fn seed_escaping_index(dir: &TempDir, root: &Path, leak_body: &str) {
        FileProjectMemory::new(root.to_path_buf())
            .init()
            .await
            .unwrap();
        fs::write(dir.path().join(".kiri").join("escape.md"), leak_body)
            .await
            .unwrap();
        let index_json = r#"{"entries":{"evil":{"path":"../escape.md","kind":"fact","tags":[],"updated_at":"2026-01-01T00:00:00Z"}}}"#;
        fs::write(root.join("index.json"), index_json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn load_skips_escaping_index_path() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        seed_escaping_index(&dir, &root, "leaked secret").await;

        let memory = FileProjectMemory::new(root);
        memory.init().await.unwrap();
        assert!(memory.load("evil").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn search_skips_escaping_index_path() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        seed_escaping_index(&dir, &root, "leaked secret token").await;

        let memory = FileProjectMemory::new(root);
        memory.init().await.unwrap();
        assert!(memory.search("leaked", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn entry_body_written_atomically() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        let entry = MemoryEntry::new(
            MemoryKind::Pattern,
            "atomic body content".into(),
            HashSet::new(),
            None,
        );
        memory.save(&entry).await.unwrap();

        // A successful save leaves no temp sibling behind (the rename consumed it).
        let mut reader = fs::read_dir(&root).await.unwrap();
        while let Some(e) = reader.next_entry().await.unwrap() {
            let name = e.file_name();
            assert!(
                !name.to_string_lossy().ends_with(".kiri-tmp"),
                "leftover temp file: {}",
                name.to_string_lossy()
            );
        }

        let loaded = memory.load(&entry.id).await.unwrap().unwrap();
        assert_eq!(loaded.content, entry.content);
    }

    /// Hand-write an index whose stored path is a plain in-root name that is actually a symlink pointing
    /// OUTSIDE the memory root, with a real secret at the link target. The path passes the lexical guard;
    /// only the canonicalize backstop keeps the read inside `root`.
    #[cfg(unix)]
    async fn seed_symlinked_index(dir: &TempDir, root: &Path, secret_body: &str) {
        FileProjectMemory::new(root.to_path_buf())
            .init()
            .await
            .unwrap();
        let secret = dir.path().join("secret.txt");
        fs::write(&secret, secret_body).await.unwrap();
        std::os::unix::fs::symlink(&secret, root.join("leak.md")).unwrap();
        let index_json = r#"{"entries":{"evil":{"path":"leak.md","kind":"fact","tags":[],"updated_at":"2026-01-01T00:00:00Z"}}}"#;
        fs::write(root.join("index.json"), index_json)
            .await
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn load_and_search_skip_symlinked_index_path() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        seed_symlinked_index(&dir, &root, "PRIVATE KEY MATERIAL").await;

        let memory = FileProjectMemory::new(root);
        memory.init().await.unwrap();
        // The symlinked entry resolves outside the memory root, so both readers must skip it.
        assert!(memory.load("evil").await.unwrap().is_none());
        assert!(memory.search("PRIVATE", 10).await.unwrap().is_empty());
    }
}
