# Audit — session module

> Scope: `src/modules/session.rs`, `src/modules/session/{application/mod.rs, application/session_store.rs, domain/mod.rs, domain/session.rs, infrastructure/mod.rs, infrastructure/message_dto.rs, infrastructure/sqlite_session_store.rs}` (read in full); cross-referenced against `src/modules/memory/infrastructure/sqlite_shared_memory.rs`, `src/modules/memory/domain/entry.rs`, `src/modules/provider/infrastructure/openai/message_dto.rs`, `src/shared/kernel/message.rs`, `src/modules/tui/infrastructure/runtime.rs`, `src/app.rs`.  
> Date: 2026-06-27  
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The session module is one of the healthier areas of the codebase: clean hexagonal layering (pure
domain, capability-named port, single SQLite adapter), fully parameterized SQL (no injection surface),
a `DB_OP_TIMEOUT` on every blocking op, defensive row/role parsing that recovers instead of panicking,
and a real test suite covering the happy paths plus reopen/persistence. No `unwrap`/`expect`/`panic`
exists on any runtime-reachable path, and call-site error handling in the TUI surfaces failures as
`Notice`s rather than swallowing them. There are no Critical or High findings. The headline issues are
all maintainability-grade: the SQLite plumbing (`run_blocking` / `lock` / `DB_OP_TIMEOUT` /
`now_rfc3339`) is duplicated almost verbatim with the memory store, the `Role`↔wire-string mapping is a
third independent copy of logic the provider already owns, and several smaller dead-code / magic-string
/ consistency nits remain.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 0 | 2 | 8 |

## Findings

### [SESS-01] Extract the duplicated SQLite plumbing shared with the memory store
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/session/infrastructure/sqlite_session_store.rs:26`, `src/modules/session/infrastructure/sqlite_session_store.rs:72`, `src/modules/session/infrastructure/sqlite_session_store.rs:79`, `src/modules/session/infrastructure/sqlite_session_store.rs:82` vs `src/modules/memory/infrastructure/sqlite_shared_memory.rs:116`, `src/modules/memory/infrastructure/sqlite_shared_memory.rs:123`, `src/modules/memory/infrastructure/sqlite_shared_memory.rs:127`, `src/modules/memory/domain/entry.rs:96`
- **Problem:** `lock`, `DB_OP_TIMEOUT`, `run_blocking`, and `now_rfc3339` are reimplemented near-verbatim in two SQLite adapters; only the error-variant constructor differs (`sess` → `AgentError::Session` vs `mem` → `AgentError::Memory`). The two `run_blocking` bodies, the `lock` mutex-poison mapping, and the `Duration::from_secs(5)` timeout are byte-for-byte the same logic. A change to the timeout policy, the join-error handling, or the poison message must be made in two places and will drift. (Aggravating: the memory copy of `now_rfc3339` lives in the *domain* layer (`entry.rs:96`), so the duplication also straddles a layer boundary — the session copy is correctly in infrastructure.)
- **Evidence:**
```rust
// sqlite_session_store.rs
async fn run_blocking<T, F>(op: F) -> Result<T>
where F: FnOnce() -> Result<T> + Send + 'static, T: Send + 'static {
    match tokio::time::timeout(DB_OP_TIMEOUT, spawn_blocking(op)).await {
        Ok(joined) => joined.map_err(sess)?,
        Err(_) => Err(AgentError::Session("database operation timed out".to_string())),
    }
}
// sqlite_shared_memory.rs — identical but for `mem` / AgentError::Memory
```
- **Recommendation:** Hoist the connection plumbing into a shared SQLite infra helper (e.g. `src/shared/infra/sqlite.rs`) parameterized by the error mapper: a generic `run_blocking<E>(timeout, map_err, op)` + `lock(conn, map_err)` + a single `DB_OP_TIMEOUT` const + an RFC3339 `now()`. Each store keeps only its `sess`/`mem` mapper. Do not implement — scan only.

### [SESS-02] Role↔wire-string mapping is a third independent copy
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/session/infrastructure/message_dto.rs:23`, `src/modules/session/infrastructure/message_dto.rs:34` vs `src/modules/provider/infrastructure/openai/message_dto.rs:105`
- **Problem:** `role_to_str` / `role_from_str` map the four `Role` variants to/from exactly the same strings (`"system"`/`"user"`/`"assistant"`/`"tool"`) that the provider's `wire_role` already maps. Two adapters now own the canonical string form of a kernel enum. If a fifth role is ever added, the compiler will catch the `match` in each, but the *string spelling* (and any future renames) can silently diverge between what is persisted to SQLite and what is sent on the wire, producing sessions that load with a role the provider would serialize differently.
- **Evidence:**
```rust
// session/infrastructure/message_dto.rs:23
pub fn role_to_str(role: Role) -> &'static str {
    match role { Role::System => "system", Role::User => "user",
                 Role::Assistant => "assistant", Role::Tool => "tool" }
}
// provider/infrastructure/openai/message_dto.rs:105
const fn wire_role(role: Role) -> &'static str { /* same four arms */ }
```
- **Recommendation:** Make the wire spelling a single source of truth on the kernel `Role` (e.g. `Role::as_wire(&self) -> &'static str` + `Role::from_wire(&str) -> Option<Role>` in `shared/kernel/role.rs`), and have both DTOs call it. Keeps the domain serde-free while removing the third copy.

### [SESS-03] `SessionStore::delete` is unused yet documented as live
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/modules/session/application/session_store.rs:39`, `src/modules/session/application/session_store.rs:12`, `src/modules/session/infrastructure/sqlite_session_store.rs:346`
- **Problem:** `delete` is annotated `#[allow(dead_code)]` and is called only from its own test (`sqlite_session_store.rs:470`); no production caller exists (verified across `src/`). Yet the trait-level doc comment states `delete` *"prunes empty/aborted sessions"* in the present tense, implying it is wired into the runtime. The doc misrepresents reality and the prune behavior it describes is never invoked, so empty/aborted sessions in fact accumulate.
- **Evidence:**
```rust
/// `init/create/.../load` are used by the
/// TUI runtime; `delete` prunes empty/aborted sessions.   // <- claims active use
...
    #[allow(dead_code)]
    async fn delete(&self, session_id: &str) -> Result<bool>;
```
- **Recommendation:** Either wire the prune (call `delete` on the empty/aborted-session path in the runtime) or, if pruning is out of scope for now, soften the doc to "reserved for the planned session-management prune" so the comment matches the `#[allow(dead_code)]` reality.

### [SESS-04] `Session` carries three load-only, unused fields
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/modules/session/domain/session.rs:10`, `src/modules/session/domain/session.rs:15`, `src/modules/session/domain/session.rs:17`
- **Problem:** `project_id`, `created_at`, and `updated_at` on `Session` are each `#[allow(dead_code)]` — populated on `load`/`create` but never read by any consumer (the resume path uses only `id`/`title`/`messages`; the picker reads `SessionSummary`). They are honestly commented as reserved for future sync/UI, but three suppressed-warning fields on the core entity is carried weight that a reader must mentally discount.
- **Evidence:**
```rust
    #[allow(dead_code)]
    pub project_id: String,
    ...
    #[allow(dead_code)]
    pub created_at: String,
    #[allow(dead_code)]
    pub updated_at: String,
```
- **Recommendation:** When the sync/session-management UI lands, wire these and drop the allows; until then this is acceptable but should be tracked so the suppressions do not become permanent. Consider whether the resume path actually needs the full `Session` or a leaner projection.

### [SESS-05] `now_rfc3339` empty-string fallback can corrupt ordering and labels
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/session/infrastructure/sqlite_session_store.rs:26`
- **Problem:** On a (practically impossible) RFC3339 format failure, `now_rfc3339` returns `""` via `unwrap_or_default()`. That empty string is written to `created_at`/`updated_at`, which drive `ORDER BY s.updated_at DESC` (`list_for_project`) and the `short_timestamp` label. An empty timestamp would silently sort unpredictably and render as a blank label rather than failing loudly. The justification comment is honest that formatting "cannot fail in practice", so this is low risk, but the fallback degrades a correctness-relevant column rather than surfacing.
- **Evidence:**
```rust
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
```
- **Recommendation:** If kept, this is fine given the impossibility argument; if hardened, propagate the format error as `AgentError::Session` from `create`/`append_messages` so a malformed timestamp can never be persisted. Scan only — no change required.

### [SESS-06] `"(sem título)"` fallback literal duplicated across domain and TUI
- **Severity:** Low
- **Category:** magic
- **Location:** `src/modules/session/domain/session.rs:40`, `src/modules/tui/infrastructure/runtime.rs:1233`, `src/modules/tui/infrastructure/runtime.rs:1297`
- **Problem:** The empty-title fallback label `"(sem título)"` is hardcoded in three production sites: `derive_title` returns it, and the `/sessions` picker independently re-derives it twice when a stored title is blank. Three copies of the same user-facing magic string mean a wording change must touch all three, and the picker re-implements a fallback the domain already provides.
- **Evidence:**
```rust
// domain/session.rs:40
    return "(sem título)".to_string();
// runtime.rs:1233 / :1297 — picker re-applies the same literal when title is blank
```
- **Recommendation:** Define the fallback once (a `pub const` next to `derive_title`, or have the picker call `derive_title`/reuse the stored title without re-inventing the blank case) so the label lives in a single place.

### [SESS-07] Inert-store modeling diverges from the sibling memory store
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/session/infrastructure/sqlite_session_store.rs:38`, `src/modules/session/infrastructure/sqlite_session_store.rs:63`, `src/modules/session/application/session_store.rs:43` vs `src/modules/memory/infrastructure/sqlite_shared_memory.rs:45`
- **Problem:** Two sibling SQLite adapters solve the same "degraded fallback when the DB cannot open" problem with different shapes: `SqliteSessionStore` carries an `available: bool` field, an `in_memory_inert()` constructor, and an `is_available()` port method; `SqliteSharedMemory` exposes `in_memory()` with no availability flag at all. A reader moving between the two modules has to relearn the convention, and a caller cannot ask the memory store whether it is degraded the way it can the session store.
- **Evidence:**
```rust
// session: explicit availability
pub fn in_memory_inert() -> Result<Self> { ... available: false ... }
fn is_available(&self) -> bool { self.available }
// memory: no availability concept
pub fn in_memory() -> Result<Self> { /* no flag */ }
```
- **Recommendation:** Pick one convention for "inert SQLite store" and apply it to both (the session module's explicit `is_available()` is the stronger model). Coordinate with the memory-module audit.

### [SESS-08] No `UNIQUE(session_id, ordinal)` guard on the deferred-transaction append
- **Severity:** Low
- **Category:** architecture
- **Location:** `src/modules/session/infrastructure/sqlite_session_store.rs:111`, `src/modules/session/infrastructure/sqlite_session_store.rs:174`, `src/modules/session/infrastructure/sqlite_session_store.rs:177`
- **Problem:** `append_messages` computes the next ordinal with `SELECT COALESCE(MAX(ordinal), -1) + 1` inside an `unchecked_transaction` (SQLite's default DEFERRED mode takes no write lock on the read). The `messages` schema has no `UNIQUE(session_id, ordinal)` constraint. `~/.kiri/sessions.db` is explicitly global across terminals (see the `busy_timeout` rationale comment), so two processes appending to the *same* `session_id` concurrently could both read the same `MAX` and insert duplicate ordinals, leaving an ambiguous `ORDER BY ordinal` on load. In normal use each running Kiri owns a distinct freshly-created session, so this is not reachable today — but the schema offers no structural guarantee.
- **Evidence:**
```sql
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,   -- no UNIQUE(session_id, ordinal)
    ...);
```
- **Recommendation:** Add `UNIQUE(session_id, ordinal)` (a duplicate insert then fails the transaction loudly instead of silently corrupting order), and/or take the append transaction `IMMEDIATE`. Document the single-writer-per-session assumption if the constraint is declined.

### [SESS-09] Corrupt-row skips on `load` are silent (no signal that the conversation is truncated)
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/session/infrastructure/sqlite_session_store.rs:310`, `src/modules/session/infrastructure/sqlite_session_store.rs:314`, `src/modules/session/infrastructure/sqlite_session_store.rs:330`
- **Problem:** When a stored `images`/`tool_calls` JSON column is unparseable, or a row's role is unknown, `load` drops the whole message and returns the session as if intact. The drop is correctly justified in-comment (skipping is safer than emptying tool_calls), satisfying the "ignore-with-justification" rule — but nothing signals to the loader or user that the resumed conversation is missing turns. A user could resume a session that quietly lost an assistant/tool exchange.
- **Evidence:**
```rust
let tool_calls = match serde_json::from_str(&tool_calls_raw) {
    Ok(value) => value,
    Err(_) => return Ok(None),   // whole message dropped, no count surfaced
};
```
- **Recommendation:** Track a skipped-row count and surface it on resume (e.g. a `Notice`: "N corrupt message(s) skipped"), so silent truncation becomes visible. The skip policy itself is sound and should stay.

### [SESS-10] `Connection::open` IO failure is mapped to `AgentError::Session`, contradicting `sess`'s own doc
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/session/infrastructure/sqlite_session_store.rs:20`, `src/modules/session/infrastructure/sqlite_session_store.rs:47`, `src/modules/session/infrastructure/sqlite_session_store.rs:44`
- **Problem:** `sess` is documented as mapping *"any non-IO failure"*, and `create_dir_all` correctly uses `?` (→ the IO variant). But `Connection::open(&db_path).map_err(sess)?` is an IO-class failure (bad path/permissions) routed through `sess` into `AgentError::Session`, blurring the IO-vs-domain error classification the helper's doc establishes. Minor, but it means a disk/permission error on open is reported as a "session" error.
- **Evidence:**
```rust
/// Map any non-IO failure (SQLite, serde, join, lock) into the kernel's session error variant.
fn sess<E: std::fmt::Display>(error: E) -> AgentError { AgentError::Session(error.to_string()) }
...
let conn = Connection::open(&db_path).map_err(sess)?;  // open is IO-class
```
- **Recommendation:** Either tighten `sess`'s doc to acknowledge it also covers SQLite-open failures, or classify open errors distinctly. Cosmetic; flag only.

## Strengths
- **Security-clean SQL:** every statement is parameterized (`params![...]`); there is no string-built SQL anywhere, so the persistence layer has no injection surface despite storing arbitrary user content.
- **Disciplined failure handling:** `DB_OP_TIMEOUT` bounds every blocking op so a wedged lock/slow disk fails fast; `load` distinguishes `QueryReturnedNoRows` (→ `Ok(None)`) from real DB errors (→ surfaced); row/role parsing recovers defensively; and there is no `unwrap`/`expect`/`panic` outside `#[cfg(test)]`.
- **Faithful, well-tested DTO boundary:** `StoredMessage` mirrors all five `Message` fields (no silent field drop), keeps the domain serde-free per ADR 0003, and is covered by round-trip + unknown-role tests; the store itself has create/append-order/list/latest/delete/reopen/inert tests.
