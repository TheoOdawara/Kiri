# Audit — Memory module

> Scope: `src/modules/memory.rs` and all of `src/modules/memory/` — application (`memory_port.rs`, `distill.rs`, `project_store.rs`, `shared_store.rs`, `project_memory.rs`, `shared_memory.rs`), domain (`entry.rs`, `project_id.rs`, `similarity.rs`), infrastructure (`file_project_memory.rs`, `sqlite_shared_memory.rs`, `docs_library.rs`, `file_project_store.rs`, `sqlite_shared_store.rs`, `test_support.rs`, `tools/{mod,recall_memory,remember,consult_docs}.rs`). Cross-checked external wiring in `app.rs`, `tui/infrastructure/runtime.rs`, and `sync/` usage of the persistence ports.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The memory module is in good shape: layering is respected (domain pure of file/net I/O, ports as capability-named traits, adapters in `infrastructure`), there is no `anyhow` off the binary edge, no stdin/stdout in the engine, and no `unwrap`/`expect`/`panic!` on any runtime-reachable path (every one found is under `#[cfg(test)]`). All I/O is timeout-bounded (`EMBED_TIMEOUT`, `DB_OP_TIMEOUT`, distiller `DEFAULT_TIMEOUT`) and best-effort failures carry justification comments. The headline issues are maintainability, not correctness: a large speculative full-CRUD surface kept alive by blanket `#[allow(dead_code)]`, four near-identical in-memory test doubles duplicated across modules, two parallel trait pairs with heavily overlapping surfaces, and a stringly-typed `scope` threaded through three files. The one substantive quality bug is that stored embedding `model`/`dim` metadata is never consulted at recall time, so switching the embedding model silently degrades recall. No Critical or High issues.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 0 | 5 | 9 |

## Findings

### [MEM-01] Deduplicate the four near-identical in-memory store test doubles
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/memory/application/memory_port.rs:230`, `src/modules/memory/application/memory_port.rs:286`, `src/modules/memory/application/project_store.rs:48`, `src/modules/memory/application/shared_store.rs:52`
- **Problem:** Four hand-rolled `Mutex<Vec<MemoryEntry>>` store doubles (`MockProjectStore`, `MockSharedStore`, `InMemoryProjectStore`, `InMemorySharedStore`) implement the same `save`/`search`/`list_by_kind`/`list_by_tag`/`list_by_project`/`is_available` bodies verbatim. The `filter(...).take(limit).cloned().collect()` pattern is copy-pasted ~12 times. Any change to the port surface must be mirrored across all four, and divergence (one double behaving subtly differently) is easy to introduce unnoticed.
- **Evidence:**
```rust
async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
    let entries = self.entries.lock().unwrap();
    Ok(entries
        .iter()
        .filter(|e| e.matches_query(query))
        .take(limit)
        .cloned()
        .collect())
}
```
- **Recommendation:** Hoist one generic in-memory double (e.g. `InMemoryStore` in `infrastructure/test_support.rs`, which is already `#[cfg(test)]`) and implement both `ProjectStore` and `SharedStore` on it (or a thin wrapper), then reuse it from all three test modules.

### [MEM-02] Speculative full-CRUD ports kept alive by blanket `#[allow(dead_code)]`
- **Severity:** Medium
- **Category:** dead-code
- **Location:** `src/modules/memory/application/project_memory.rs:12`, `src/modules/memory/application/shared_memory.rs:11`, `src/modules/memory/domain/entry.rs:28`, `src/modules/memory/domain/entry.rs:125`, `src/modules/memory/domain/entry.rs:132`
- **Problem:** `ProjectMemory` and `SharedMemory` carry a trait-wide `#[allow(dead_code)]` justified as "reserved for the future memory-management UI." Cross-checking real callers: the agent loop / store adapters use only `init`/`save`/`search`/`list_by_kind`/`list_by_tag`/`load`, and `sync` uses only `SharedMemory::{list,load,save}`. The rest — `ProjectMemory::{delete,count,list}`, `SharedMemory::{delete,count,count_by_project,list_by_kind,list_by_tag,list_by_project}`, plus `MemoryKind::all`, `MemoryEntry::update_content`, `MemoryEntry::add_tags` — are exercised only by tests. A *blanket* allow on the whole trait is worse than per-method allows: because several methods are now genuinely live, the attribute no longer flags any method that becomes dead in the future. This is speculative generality (YAGNI) ahead of a UI that does not exist.
- **Evidence:**
```rust
#[allow(dead_code)]
#[async_trait]
pub trait ProjectMemory: Send + Sync {
    async fn init(&self) -> Result<()>;
    async fn save(&self, entry: &MemoryEntry) -> Result<()>;
    async fn load(&self, id: &str) -> Result<Option<MemoryEntry>>;
    async fn delete(&self, id: &str) -> Result<bool>;   // tests only
    ...
    async fn count(&self) -> Result<usize>;              // tests only
}
```
- **Recommendation:** Trim the ports to the methods that have a live caller, and move the rest behind the actual UI feature when it lands. If kept, replace the trait-wide allow with per-method `#[allow(dead_code)]` so the linter still catches newly-dead methods.

### [MEM-03] Parallel store trait pairs duplicate surface and embedding defaults
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/memory/application/project_store.rs:27`, `src/modules/memory/application/shared_store.rs:31`, `src/modules/memory/application/project_memory.rs:14`, `src/modules/memory/application/shared_memory.rs:13`
- **Problem:** `ProjectStore` and `SharedStore` are identical except `SharedStore` adds `list_by_project`; `ProjectMemory` and `SharedMemory` are identical except `SharedMemory` adds `list_by_project` + `count_by_project`. The two embedding default-methods (`save_embedding`, `embedded_candidates`) are copy-pasted byte-for-byte (same body, same doc comment) into both `ProjectStore` and `SharedStore`. Four traits encode essentially one capability set, so every signature/doc change must be made in 2–4 places.
- **Evidence:** Identical in `project_store.rs` and `shared_store.rs`:
```rust
/// Entries that carry a stored embedding, paired with their vector, up to `limit`. Default empty so a
/// non-embedding store transparently falls back to keyword recall.
async fn embedded_candidates(&self, _limit: usize) -> Result<Vec<(MemoryEntry, Vec<f32>)>> {
    Ok(Vec::new())
}
```
- **Recommendation:** Factor the common surface into one base trait (e.g. `MemoryStore` with save/search/list_by_kind/list_by_tag/embedding defaults) and have `SharedStore` extend it with the project-scoped methods, or collapse the project/shared distinction into a single store parameterized by scope. Same for the `ProjectMemory`/`SharedMemory` pair.

### [MEM-04] `scope` is stringly-typed and its project-id mapping is duplicated
- **Severity:** Medium
- **Category:** magic
- **Location:** `src/modules/memory/infrastructure/tools/recall_memory.rs:102`, `src/modules/memory/infrastructure/tools/remember.rs:104`, `src/modules/memory/application/distill.rs:143`, `src/modules/memory/application/distill.rs:159`
- **Problem:** The scope is a bare `String` matched against the literals `"project"`/`"shared"`/`"both"` in three files, with the validation logic re-implemented each time. Worse, the "shared ⇒ global `project_id` is `None`, else stamp the current project" rule is duplicated verbatim between `remember.rs` and `distill.rs::persist`. Stringly-typed control flow is exactly the "no magic / explicit & type-safe" smell the contract calls out; a typo in one literal (or a new scope) is a silent behavior bug.
- **Evidence:** duplicated in `remember.rs:104` and `distill.rs:159`:
```rust
let project_id = if scope == "shared" {
    None
} else {
    Some(self.project_id.clone())
};
```
- **Recommendation:** Introduce a `Scope` domain enum (`Project | Shared` plus a recall-only `Both`) with a single `parse`/`FromStr`, and a `project_id_for(scope, current)` helper. Tools and the distiller parse once and match exhaustively.

### [MEM-05] Embedding `model`/`dim` metadata is stored but never used at recall — model switches silently degrade recall
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/memory/infrastructure/sqlite_shared_memory.rs:75`, `src/modules/memory/infrastructure/sqlite_shared_memory.rs:194`, `src/modules/memory/application/memory_port.rs:109`
- **Problem:** Every embedding is persisted with its `model` (and SQLite also stores `dim`), but `embedded_candidates` selects only the vector and `semantic_pick` ranks the current query's embedding against *all* stored vectors regardless of which model produced them. `cosine` returns `0.0` only on a length mismatch, so vectors from a *different* model with the *same* dimensionality (e.g. two 1536-dim models) are ranked on meaningless cross-model cosines that can clear `MIN_SIMILARITY`. The `MIN_SIMILARITY` doc-comment claims this is "blunted," but that only holds for different-dimension models. Net effect: changing the embedding provider/model silently corrupts semantic recall with no re-embed and no filter, even though the metadata needed to guard it is already on disk.
- **Evidence:**
```rust
// sqlite_shared_memory.rs — model is written...
"INSERT INTO entry_embeddings (entry_id, model, dim, vector) ..."
// ...but embedded_candidates never selects or filters on it:
"SELECT e.id, ..., emb.vector FROM entries e JOIN entry_embeddings emb ...
 ORDER BY e.updated_at DESC LIMIT ?1"
```
- **Recommendation:** Filter `embedded_candidates` to the current embedder's `model()` (the column exists for exactly this), or treat a model change as a re-embed trigger. At minimum, document that mixed-model stores rank on cross-model cosines.

### [MEM-06] Inline magic numbers in the distiller's dedup
- **Severity:** Low
- **Category:** magic
- **Location:** `src/modules/memory/application/distill.rs:181`, `src/modules/memory/application/distill.rs:186`, `src/modules/memory/application/distill.rs:216`
- **Problem:** `leading_words(content, 6)`, `recall_*(&query, 5)`, and the Jaccard `>= 0.8` threshold are bare literals inside `is_duplicate`/`is_near_duplicate`. The module otherwise names its tunables well (`SEMANTIC_CANDIDATES`, `MIN_SIMILARITY`, `DEFAULT_MAX_ENTRIES`, …), so these stand out as unexplained knobs governing whether knowledge is dropped as a duplicate.
- **Evidence:**
```rust
let query = leading_words(content, 6);
...
"shared" => self.memory.recall_shared(&query, 5).await,
...
(intersection as f32 / union as f32) >= 0.8
```
- **Recommendation:** Promote to named `const`s (e.g. `DEDUP_QUERY_WORDS`, `DEDUP_RECALL_LIMIT`, `NEAR_DUPLICATE_JACCARD`) alongside the existing distiller constants.

### [MEM-07] `load`/`search` trust the stored relative path without re-validating it stays under root
- **Severity:** Low
- **Category:** security
- **Location:** `src/modules/memory/infrastructure/file_project_memory.rs:238`, `src/modules/memory/infrastructure/file_project_memory.rs:269`
- **Problem:** Both `load` and `search` build a read path by joining `index_entry.path` (read from `index.json`) onto `self.root` and read it with no check that the result is still inside `root`. The `index.json` is harness-owned (low risk), but it is an on-disk file an external tool or a corrupted/merged sync could rewrite; a `path` of `../../something` would read outside the memory directory. The write side correctly derives the relative path under `root`, but the read side never re-asserts that invariant.
- **Evidence:**
```rust
let path = self.root.join(&index_entry.path);
drop(index);
let content = fs::read_to_string(&path).await?;
```
- **Recommendation:** After joining, canonicalize and assert the path is a descendant of `root` (reuse the sandbox's containment check), skipping/erroring on any entry that escapes.

### [MEM-08] `DocsLibrary` directory walk follows symlinks, allowing escape from the docs root
- **Severity:** Low
- **Category:** security
- **Location:** `src/modules/memory/infrastructure/docs_library.rs:84`, `src/modules/memory/infrastructure/docs_library.rs:97`
- **Problem:** `collect_markdown_files` descends with `path.is_dir()` (which follows symlinks) and reads any matching file with `fs::read`. A symlink placed under `docs/` that points outside the workspace (e.g. to `~`) is silently followed, so `consult_docs` can surface excerpts of files outside the intended docs tree. The `MAX_FILE_BYTES`/`MAX_FILES_SCANNED` caps bound the volume but not the boundary.
- **Evidence:**
```rust
let path = entry.path();
if path.is_dir() {
    dirs.push(path);
} else if is_markdown(&path) {
    files.push(path);
```
- **Recommendation:** Use `symlink_metadata`/`file_type().is_symlink()` to skip symlinked entries, or canonicalize each candidate and confirm it stays under `root` before reading.

### [MEM-09] Recall short-circuits semantic when it fills `limit`, contradicting the "never shadowed" doc-comment
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/memory/application/memory_port.rs:152`, `src/modules/memory/application/memory_port.rs:155`
- **Problem:** `recall_project`/`recall_shared` return the semantic set early when `semantic.len() >= limit`, skipping the keyword union entirely. The comment immediately below promises "a strong keyword match — or an entry that has no embedding — is never shadowed by the semantic set," but that guarantee only holds when fewer than `limit` semantic hits exist. With a small `limit` (the tool default is 5) a fully-semantic result can shadow an exact keyword match or an un-embedded entry — the stated invariant is not actually upheld in that case.
- **Evidence:**
```rust
if semantic.len() >= limit {
    return Ok(semantic);
}
// Union with keyword recall so a strong keyword match — or an entry that has no embedding — is
// never shadowed by the semantic set, and the floor's rejects are backfilled.
```
- **Recommendation:** Either always union semantic+keyword and then truncate (accepting a second store query), or soften the comment to state the union is best-effort only when the semantic set underfills `limit`.

### [MEM-10] Entry markdown is written non-atomically while the index/sidecar use atomic writes
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/memory/infrastructure/file_project_memory.rs:213`, `src/modules/memory/infrastructure/file_project_memory.rs:21`
- **Problem:** `save` persists the entry `.md` body with a plain `fs::write` (no temp-then-rename), yet `save_index`/`save_embedding` go through `write_atomic` precisely because "a crash mid-write can otherwise truncate" them. The entry body has the same exposure: a crash mid-write leaves a half-written file. Additionally the ordering (write file → mutate index → `save_index`) means a crash after the body write but before `save_index` orphans the file (present on disk, absent from the index, hence invisible). The durability guarantee is inconsistent within one adapter.
- **Evidence:**
```rust
fs::write(&path, content).await?;          // not atomic
...
self.save_index().await?;                  // atomic via write_atomic
```
- **Recommendation:** Route the entry-body write through `write_atomic` too (generalize it beyond `.json`), and document the file-before-index ordering as the recovery contract (orphan files are simply skipped by `search`).

### [MEM-11] `MemoryKind::from_str` shadows the standard `FromStr` trait
- **Severity:** Low
- **Category:** naming
- **Location:** `src/modules/memory/domain/entry.rs:53`
- **Problem:** An inherent `pub fn from_str(s: &str) -> Option<Self>` collides in name with `std::str::FromStr::from_str` but has a different signature (`Option` vs `Result`) and is not the trait. A reader who reaches for `"fact".parse::<MemoryKind>()` gets a compile error, and the inherent method can be accidentally shadowed by a trait import. It is called in several hot spots (`row_to_entry`, `remember`, `distill`), so the ambiguity is load-bearing.
- **Evidence:**
```rust
pub fn from_str(s: &str) -> Option<Self> {
    match s { "decision" => Some(MemoryKind::Decision), ... _ => None }
}
```
- **Recommendation:** Implement `std::str::FromStr` (with an `AgentError`/unit error) and/or rename the inherent helper to `parse`/`try_from_str` to avoid the std collision.

### [MEM-12] `parse_markdown_file`/`render_markdown_file` are methods that ignore `self` (and an unused `_path`)
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/modules/memory/infrastructure/file_project_memory.rs:170`, `src/modules/memory/infrastructure/file_project_memory.rs:190`
- **Problem:** Both are `&self` methods that never read any field of `self`; `parse_markdown_file` additionally takes a `_path: &Path` parameter it never uses. They are pure functions wearing a method signature, which obscures that they have no dependency on the store's state and forces callers to hold a `&self`.
- **Evidence:**
```rust
fn parse_markdown_file(&self, _path: &Path, content: &str) -> Result<MemoryEntry> {
    // ...self never used; _path never used...
}
```
- **Recommendation:** Make them free functions `parse_markdown_file(content: &str)` / `render_markdown_file(entry: &MemoryEntry)` and drop the dead `_path` parameter.

### [MEM-13] Domain constructor performs ambient clock + RNG side effects
- **Severity:** Low
- **Category:** architecture
- **Location:** `src/modules/memory/domain/entry.rs:96`, `src/modules/memory/domain/entry.rs:110`
- **Problem:** `MemoryEntry::new` (and the helper `now_rfc3339`) reads the wall clock (`OffsetDateTime::now_utc`) and draws randomness/clock via `Uuid::now_v7`. The architecture states `domain` is "pure data/rules, no I/O"; clock and RNG are ambient side effects that make the constructor non-deterministic and harder to test (the existing test even `sleep`s 10ms to force a timestamp change). It is a pragmatic and common exception, but it is the one spot where the domain layer reaches outside itself.
- **Evidence:**
```rust
pub fn new(...) -> Self {
    let id = Uuid::now_v7().to_string();
    let timestamp = now_rfc3339();           // OffsetDateTime::now_utc()
    ...
}
```
- **Recommendation:** If strict domain purity matters, inject the timestamp + id (e.g. a `Clock`/`IdGen` passed from the application layer), or explicitly document this as an accepted domain exception in the module's ADR so it is not flagged repeatedly.

### [MEM-14] Embedding persistence bypasses the persistence ports as inherent methods; `EMBED_TIMEOUT` doc understates its reuse
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/memory/infrastructure/file_project_memory.rs:105`, `src/modules/memory/infrastructure/sqlite_shared_memory.rs:53`, `src/modules/memory/application/memory_port.rs:19`
- **Problem:** Every other operation flows through the `ProjectMemory`/`SharedMemory` ports and is delegated by the store adapters; embeddings break the pattern — `save_embedding`/`embedded_candidates` are inherent `pub async fn` on the concrete `FileProjectMemory`/`SqliteSharedMemory` and live as *defaults* on the `ProjectStore`/`SharedStore` traits instead, so the persistence port (`ProjectMemory`/`SharedMemory`) has no notion of embeddings at all. The two surfaces for the same data are easy to drift. Separately, `EMBED_TIMEOUT` is documented as bounding "the query-embedding call," but `embed_query` is reused at write time to embed entry content (`remember_project`/`remember_shared`), so the constant also bounds write-path embedding — the doc undersells its scope.
- **Evidence:**
```rust
// memory_port.rs:19 — doc says "query-embedding call" only...
const EMBED_TIMEOUT: Duration = Duration::from_secs(5);
// ...but also used on the write path:
&& let Some(vector) = embed_query(embedder.as_ref(), &content).await
```
- **Recommendation:** Decide on one home for embedding persistence — either add it to the `*Memory` ports (consistent with everything else) or keep it adapter-inherent but drop the redundant trait defaults — and widen the `EMBED_TIMEOUT` doc to "any single embed call (query or content)."

## Strengths
- Error handling is genuinely production-grade for this surface: all I/O is timeout-bounded (`EMBED_TIMEOUT`, `DB_OP_TIMEOUT`, distiller `DEFAULT_TIMEOUT`), every best-effort `unwrap_or_default`/`let _ =` carries a justification comment, and there is no `unwrap`/`expect`/`panic!` reachable outside `#[cfg(test)]`.
- Defensive persistence: atomic index/sidecar writes (`write_atomic`), per-file resilience in `search`/`embedded_candidates` (one corrupt file never blanks the result), full-id filenames to avoid UUID-v7 millisecond collisions, and char-boundary-safe slicing in `render_transcript`/`excerpt_around`.
- Clean hexagonal layering and good test coverage — pure ranking math (`similarity.rs`) is unit-tested in isolation, semantic recall is verified end-to-end with a deterministic fake embedder, and the SQLite `LIKE` queries are parameter-bound (no injection).
