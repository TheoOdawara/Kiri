# Audit — Cross-module duplication

> Scope: the whole `src/` tree, read in full, focusing on logic/structure repeated ACROSS modules (within-module duplication is left to each module's own pass). Special attention to provider `openai` vs `anthropic` parallel structures, `session` vs `kernel` message mapping, SQLite boilerplate across `memory` + `session`, repeated path/dir/atomic-write helpers, repeated error mappers, and repeated test scaffolding.  
> Date: 2026-06-27  
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The codebase is, on the whole, well-factored: genuinely shared concerns already live in one place — `http_error::error_from_status` (provider 4xx/5xx classification), `tools/infrastructure/args` + `tools/application/tool` (tool scaffolding), `tools/infrastructure/support` (fs helpers), and `ProviderKind::requires_api_key` (the single auth-rule source). The remaining duplication is concentrated in two zones the whole-graph view exposes: (1) the two SQLite stores in `memory` and `session` carry a near-identical blocking-connection harness (`run_blocking`/`lock`/`DB_OP_TIMEOUT`/error-mapper/open-with-parent), and (2) the two provider adapters (`openai`, `anthropic`) carry parallel `TurnAccumulator`s, an identical streaming/status-read loop, and three independent copies of the "JSON-args-or-fallback-to-`{}`" rule. A protocol-critical detail — the canonical `Role`↔wire-string mapping — exists in three independent copies across `provider` and `session`, the one place a silent drift would corrupt stored/sent history. None of this is a correctness bug today; it is maintainability debt where a future change must be made in N places or risk divergence.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 2 | 5 | 6 |

## Findings

### [DUP-01] Unify the SQLite blocking-store harness shared by `memory` and `session`
- **Severity:** High
- **Category:** duplication
- **Location:** `src/modules/memory/infrastructure/sqlite_shared_memory.rs:116`–`138`, `src/modules/session/infrastructure/sqlite_session_store.rs:72`–`93` (plus the `mem`/`sess` mappers at `sqlite_shared_memory.rs:16` / `sqlite_session_store.rs:20`, and the open-with-parent in each `new` at `sqlite_shared_memory.rs:33` / `sqlite_session_store.rs:43`)
- **Problem:** Both SQLite adapters reimplement the same blocking-connection harness: a `lock(conn)` that maps a poisoned mutex, a `const DB_OP_TIMEOUT: Duration = Duration::from_secs(5)`, an `async fn run_blocking` that wraps `spawn_blocking` in a `tokio::time::timeout`, and a `new(db_path)` that `create_dir_all`s the parent then `Connection::open`s. The only differences are the `AgentError` variant (`Memory` vs `Session`) and the session store's extra `busy_timeout`/WAL pragmas. This is the single largest cross-module copy in the tree; a change to the timeout policy, the poisoned-lock message, or the join-error handling must be made twice and kept in sync by hand.
- **Evidence:**
```rust
// sqlite_shared_memory.rs (Memory variant)               // sqlite_session_store.rs (Session variant)
const DB_OP_TIMEOUT: Duration = Duration::from_secs(5);   const DB_OP_TIMEOUT: Duration = Duration::from_secs(5);
async fn run_blocking<T, F>(op: F) -> Result<T> ... {     async fn run_blocking<T, F>(op: F) -> Result<T> ... {
    match tokio::time::timeout(DB_OP_TIMEOUT,                 match tokio::time::timeout(DB_OP_TIMEOUT,
        spawn_blocking(op)).await {                              spawn_blocking(op)).await {
        Ok(joined) => joined.map_err(mem)?,                      Ok(joined) => joined.map_err(sess)?,
        Err(_) => Err(AgentError::Memory(                       Err(_) => Err(AgentError::Session(
            "database operation timed out".to_string())),           "database operation timed out".to_string())),
    } }                                                       } }
```
- **Recommendation:** Extract a shared blocking-SQLite support module (e.g. `shared/infra/sqlite.rs`, alongside `config`, which already owns harness file/dir I/O). Provide `open_with_parent(path) -> Connection`, a generic `run_blocking<T>(timeout, error_ctor, op)` and `lock(conn, error_ctor)` parameterized by an `Fn(String) -> AgentError`, and the `DB_OP_TIMEOUT` const. Each store keeps only its schema, its queries, and its chosen error constructor + pragmas. Do NOT implement — scan-only.

### [DUP-04] Collapse the three copies of the canonical `Role`↔wire-string mapping
- **Severity:** High
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/openai/message_dto.rs:105`–`112` (`wire_role`), `src/modules/session/infrastructure/message_dto.rs:23`–`30` (`role_to_str`) and `:34`–`42` (`role_from_str`)
- **Problem:** The kernel `Role` enum is deliberately serde-free, so each consumer maps it to the canonical lowercase wire string itself. But the same four-string mapping (`System=>"system"`, `User=>"user"`, `Assistant=>"assistant"`, `Tool=>"tool"`) now exists in three independent copies across two modules: the OpenAI send DTO, the session store's serialize, and the session store's parse (the inverse). This is the protocol detail most dangerous to let drift — the session store persists these strings to SQLite and the provider sends them on the wire; if one copy ever gains/renames a variant and another does not, stored history and sent history disagree silently. (Anthropic's mapping at `anthropic/message_dto.rs:89` is intentionally different — `Assistant` vs `user`-for-everything — and is correctly NOT part of this set.)
- **Evidence:**
```rust
// openai/message_dto.rs                          // session/infrastructure/message_dto.rs
const fn wire_role(role: Role) -> &'static str {  pub fn role_to_str(role: Role) -> &'static str {
    match role {                                      match role {
        Role::System => "system",                         Role::System => "system",
        Role::User => "user",                             Role::User => "user",
        Role::Assistant => "assistant",                   Role::Assistant => "assistant",
        Role::Tool => "tool",                             Role::Tool => "tool",
    } }                                               } }
```
- **Recommendation:** Add `Role::as_wire_str(self) -> &'static str` (a `const fn`, not a serde impl — keeps the domain serde-free) and `Role::from_wire_str(&str) -> Option<Role>` on the kernel `Role` type, and have both the OpenAI DTO and the session store call them. The canonical strings then live in one place.

### [DUP-02] Extract the shared provider stream-and-status loop
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/openai/provider.rs:87`–`117`, `src/modules/provider/infrastructure/anthropic/provider.rs:91`–`118`, and the non-success body read also at `src/modules/provider/infrastructure/openai/embeddings.rs:96`–`103`
- **Problem:** Both chat adapters run an identical post-response sequence: read `status`, on non-success read the body via `response.text().await.unwrap_or_else(|error| format!("<error body unavailable: {error}>"))` and return `error_from_status`, then `let stream = response.bytes_stream().eventsource()`, `tokio::pin!`, drive `while let Some(event)` mapping a read error to `AgentError::Provider(format!("error reading stream: {error}"))`, feed `handle_event`, check `accumulator.hit_empty_output_limit()`, and `Ok(accumulator.into_completed())`. The `<error body unavailable>` body-read block alone is copied verbatim three times (the two chat adapters plus embeddings). Only the empty-output-limit message and the URL differ.
- **Evidence:**
```rust
// identical in openai/provider.rs and anthropic/provider.rs:
let body = response.text().await
    .unwrap_or_else(|error| format!("<error body unavailable: {error}>"));
return Err(error_from_status(status, body));
...
let stream = response.bytes_stream().eventsource();
tokio::pin!(stream);
while let Some(event) = stream.next().await {
    let event = event.map_err(|error| AgentError::Provider(format!("error reading stream: {error}")))?;
    handle_event(&event.data, &mut accumulator, sink)?;
}
```
- **Recommendation:** Add two helpers in `provider/infrastructure` (next to `http_error`): `read_error_body(response) -> String` (the `<error body unavailable>` reader) shared by all three adapters, and a `stream_completion(response, &mut accumulator, sink, on_event=handle_event)` that owns the eventsource loop and the read-error mapping. The empty-output-limit message stays per-adapter. Do NOT implement — scan-only.

### [DUP-03] Factor the parallel `TurnAccumulator`s in `openai/sse.rs` and `anthropic/sse.rs`
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/openai/sse.rs:128`–`198`, `src/modules/provider/infrastructure/anthropic/sse.rs:81`–`159`
- **Problem:** Each SSE module defines its own `TurnAccumulator` with the same shape and responsibilities: a `content: String`, a `BTreeMap<u32, Partial…>` of tool calls keyed by index (assembled from streamed slices), a finish/stop reason, a `hit_empty_output_limit()` that differs only by the sentinel (`"length"` vs `"max_tokens"`), and an `into_completed()` that drains the map into `Vec<ToolCall>` with `kind` defaulting to `"function"`. The per-fragment ingestion differs (OpenAI fragments carry `id/type/name/args`; Anthropic splits `content_block_start` from `input_json_delta`), but the index-keyed assembly, the empty-output gate, and the `into_completed` shape are the same idea written twice.
- **Evidence:**
```rust
// openai/sse.rs                                    // anthropic/sse.rs
pub(crate) fn hit_empty_output_limit(&self) ->      pub(crate) fn hit_empty_output_limit(&self) ->
    bool {                                              bool {
    self.finish_reason.as_deref() == Some("length")    self.stop_reason.as_deref() == Some("max_tokens")
        && self.content.is_empty()                          && self.content.is_empty()
        && self.tool_calls.is_empty()                       && self.tool_uses.is_empty()
}                                                   }
```
- **Recommendation:** Extract a shared index-keyed tool-call assembler + `into_completed`-style builder (it produces the kernel `ToolCall` either way), and a small `empty_output_limit(reason_matches, content, calls)` predicate the two accumulators delegate to. Keep the provider-specific fragment ingestion in each module. Do NOT implement — scan-only.

### [DUP-05] Single-source the `now_rfc3339()` timestamp helper
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/memory/domain/entry.rs:94`–`100`, `src/modules/session/infrastructure/sqlite_session_store.rs:24`–`30`
- **Problem:** The exact same function — body and doc comment identical, down to the "Formatting a valid UTC instant cannot fail in practice" rationale — is defined in two modules. It is the harness's one canonical "now as RFC3339" routine, copied rather than shared. (Secondary note for the domain pass: its presence in `memory/domain/entry.rs` also reads the wall clock from the `domain` layer, which the architecture marks as no-I/O.)
- **Evidence:**
```rust
// byte-identical in entry.rs and sqlite_session_store.rs:
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
```
- **Recommendation:** Move `now_rfc3339()` to a shared home both consumers can reach (e.g. a tiny `shared/kernel` clock helper, or `shared/infra`), and have `memory` and `session` call it. This also lets the domain-layer-clock concern be resolved in one place.

### [DUP-06] Single-source the atomic-write (temp-then-rename) helper
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/memory/infrastructure/file_project_memory.rs:21`–`26`, `src/modules/sync/application/sync_service.rs:239`–`248`
- **Problem:** Two modules implement the same crash-safe write idiom — write a sibling temp file then `fs::rename` over the target — with the same motivating comment ("a crash mid-write can otherwise truncate/leave a corrupt file"). The only difference is the temp-name scheme (`with_extension("json.tmp")` vs `.{name}.kiri-tmp`). Both use `tokio::fs`.
- **Evidence:**
```rust
// file_project_memory.rs                         // sync_service.rs
async fn write_atomic(path, content) {            async fn write_atomic(path, contents) {
    let tmp = path.with_extension("json.tmp");        let tmp = path.with_file_name(format!(".{name}.kiri-tmp"));
    fs::write(&tmp, content).await?;                  fs::write(&tmp, contents).await?;
    fs::rename(&tmp, path).await?; Ok(())            fs::rename(&tmp, path).await?; Ok(())
}                                                 }
```
- **Recommendation:** Provide one shared async `write_atomic(path, bytes)` (with a single, deterministic temp-name policy) in a shared fs helper that the `memory`, `sync` (and potentially the `0600` secrets) writers reuse. Do NOT implement — scan-only.

### [DUP-07] Replace the per-module `<E: Display> -> AgentError::Variant` mappers with typed constructors
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/memory/infrastructure/sqlite_shared_memory.rs:16`, `src/modules/memory/infrastructure/file_project_memory.rs:14`, `src/modules/session/infrastructure/sqlite_session_store.rs:20`, `src/modules/sync/infrastructure/memory_ndjson.rs:25`
- **Problem:** The same one-liner — a private `fn name<E: std::fmt::Display>(error: E) -> AgentError { AgentError::Variant(error.to_string()) }` — is hand-rolled four times under three names (`mem` twice, `sess`, `ser`). Each is just "stringify any error into this module's `AgentError` variant"; the kernel error type exposes no such constructor, so every infra adapter writes its own.
- **Evidence:**
```rust
fn mem<E: std::fmt::Display>(error: E) -> AgentError { AgentError::Memory(error.to_string()) }   // ×2
fn sess<E: std::fmt::Display>(error: E) -> AgentError { AgentError::Session(error.to_string()) }
fn ser<E: std::fmt::Display>(error: E) -> AgentError { AgentError::Memory(error.to_string()) }
```
- **Recommendation:** Add typed constructors on `AgentError` in `shared/kernel/error.rs` — e.g. `AgentError::memory(impl Display)`, `AgentError::session(impl Display)`, `AgentError::sync(impl Display)` — and drop the four private helpers. The mapping rule then lives next to the variant it builds.

### [DUP-08] Share the `NullSink`/`CollectSink` `EventSink` doubles
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/memory/application/distill.rs:21`–`27` (production), `src/modules/provider/infrastructure/openai/provider.rs:132`, `src/modules/provider/infrastructure/anthropic/provider.rs:182`, `src/modules/provider/infrastructure/unconfigured.rs:44` (tests); `CollectSink` at `src/modules/provider/infrastructure/openai/sse.rs:213` and `src/modules/provider/infrastructure/anthropic/sse.rs:166`
- **Problem:** A discard-everything `EventSink` (`struct NullSink; impl EventSink { on_event -> Ok(()) }`) is written five times — once as real production code in the distiller (which legitimately streams headless) and four times as test scaffolding. The collecting test sink `CollectSink(Vec<StreamEvent>)` is likewise copied in both SSE test modules.
- **Evidence:**
```rust
// distill.rs (production) — and re-typed in 3 provider test modules:
struct NullSink;
impl EventSink for NullSink {
    fn on_event(&mut self, _event: StreamEvent) -> Result<()> { Ok(()) }
}
```
- **Recommendation:** Provide one production `NullSink` (or `DiscardSink`) next to the `EventSink` port in `provider/application/completion_provider.rs`; the distiller and every test reuse it. Optionally expose a shared `CollectSink` test helper for the SSE tests.

### [DUP-09] Centralize the keyless-bearer + base-URL-join provider idiom
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/openai/provider.rs:75`,`77`–`81`, `src/modules/provider/infrastructure/openai/embeddings.rs:78`,`80`–`84`, and the URL join at `src/modules/provider/infrastructure/anthropic/provider.rs:80`
- **Problem:** Two pieces repeat across the OpenAI-compatible adapters: the `format!("{}/<path>", self.base_url.trim_end_matches('/'))` URL join (three sites: `/chat/completions`, `/embeddings`, `/v1/messages`), and the "omit `Authorization` for a keyless endpoint, else `bearer_auth(key.expose())`" block (chat + embeddings, with the same justifying comment about empty `Bearer ` being rejected by local servers).
- **Evidence:**
```rust
// openai/provider.rs and openai/embeddings.rs, verbatim:
if let Some(key) = &self.api_key {
    request = request.bearer_auth(key.expose());
}
// and: format!("{}/embeddings", self.base_url.trim_end_matches('/'));
```
- **Recommendation:** Add small helpers in `provider/infrastructure` — `join_url(base, suffix)` and `apply_optional_bearer(request, &Option<Secret>)` — reused by the chat and embeddings adapters (the Anthropic adapter can reuse `join_url` too).

### [DUP-10] Consider a shared `AgentResult<T>` alias instead of 15 local copies
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `type Result<T> = std::result::Result<T, AgentError>;` declared in 15 files, e.g. `src/modules/memory/infrastructure/sqlite_shared_memory.rs:13`, `src/modules/session/infrastructure/sqlite_session_store.rs:17`, `src/modules/sync/infrastructure/memory_ndjson.rs:10`, `src/modules/memory/application/*` (multiple)
- **Problem:** The same private `Result<T>` shadow alias is re-declared in fifteen modules. It is a benign idiom, but it is fifteen copies of one line, and it shadows `std::result::Result` locally (a reader must check each file's alias). `config.rs` and the binary edge instead use `anyhow::Result`, so the codebase already mixes two "`Result`" meanings file-to-file.
- **Evidence:**
```rust
type Result<T> = std::result::Result<T, AgentError>;   // repeated in 15 modules
```
- **Recommendation:** Expose one `pub type AgentResult<T> = Result<T, AgentError>;` from `shared/kernel/error.rs` and use it explicitly where the engine layers want it, rather than fifteen local `Result<T>` shadows. Low priority — style/consistency, not a defect.

### [DUP-11] Hoist the duplicated `fn sandbox() -> FsSandbox` test helper into `memory` test_support
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/memory/infrastructure/tools/recall_memory.rs:158`–`160`, `src/modules/memory/infrastructure/tools/remember.rs:151`–`153`, `src/modules/memory/infrastructure/tools/consult_docs.rs:115`–`117`
- **Problem:** The three memory-tool test modules each define a byte-identical `fn sandbox() -> FsSandbox { FsSandbox::new(std::path::PathBuf::from("."), SensitiveMatcher::empty()).unwrap() }`. The `memory/infrastructure/test_support.rs` helper module already exists precisely to share memory-tool test fixtures (`temp_port`, `call`), so this third fixture belongs there too. (A near-identical helper also recurs in `tools/infrastructure/fs/list_dir.rs:146`.)
- **Evidence:**
```rust
fn sandbox() -> FsSandbox {
    FsSandbox::new(std::path::PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
}
```
- **Recommendation:** Move this `sandbox()` builder into `memory/infrastructure/test_support.rs` next to `temp_port`/`call`, and have the three tool tests import it.

### [DUP-12] Reuse the canonical default tool-call `"function"` kind instead of the scattered literal
- **Severity:** Low
- **Category:** magic
- **Location:** kernel source at `src/shared/kernel/tool_call.rs:21`–`23` (`default_function_type`), vs. the hardcoded literal at `src/modules/provider/infrastructure/openai/sse.rs:183`, `src/modules/provider/infrastructure/anthropic/sse.rs:132`, `src/modules/provider/infrastructure/openai/message_dto.rs:190`, `src/modules/provider/infrastructure/anthropic/message_dto.rs:237` (and ~13 further sites)
- **Problem:** The kernel already names the canonical default tool-call kind once (`default_function_type() -> "function".to_string()`), yet `"function".to_string()` is re-typed as a bare literal across both provider adapters' assembly/DTO code (and widely in tests). The SSE accumulators that mint `ToolCall`s default `kind` to this literal rather than the kernel's single source.
- **Evidence:**
```rust
// shared/kernel/tool_call.rs — the canonical source:
fn default_function_type() -> String { "function".to_string() }
// re-typed in openai/sse.rs into_completed(): kind: if partial.kind.is_empty() { "function".to_string() } ...
// re-typed in anthropic/sse.rs into_completed(): kind: "function".to_string(),
```
- **Recommendation:** Expose the canonical kind as a kernel `pub const TOOL_CALL_FUNCTION_KIND: &str = "function";` (or a public `ToolCall::FUNCTION_KIND`) and have the SSE accumulators and DTOs reference it, so the protocol string lives in exactly one place.

### [DUP-13] Consolidate the three "JSON-args-or-fallback-to-`{}`" normalizers across the provider adapters
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/openai/arguments.rs:82`–`89` (`normalize_arguments`), `src/modules/provider/infrastructure/anthropic/sse.rs:149`–`159` (`tool_input_to_arguments`), `src/modules/provider/infrastructure/anthropic/message_dto.rs:149`–`155` (`parse_input`)
- **Problem:** Three functions across the two provider adapters encode the same defensive rule — "trim the assembled tool-call arguments; an empty string or non-JSON falls back to an empty object so a garbled turn can never poison a later request." `normalize_arguments` (OpenAI) and `tool_input_to_arguments` (Anthropic SSE) both return a `String` (`"{}"` fallback); `parse_input` (Anthropic DTO) returns a `serde_json::Value` (`json!({})` fallback). The validation logic and the same rationale comment are written three times.
- **Evidence:**
```rust
// anthropic/sse.rs                                  // anthropic/message_dto.rs
fn tool_input_to_arguments(input: String) -> String { fn parse_input(arguments: &str) -> Value {
    let trimmed = input.trim();                           let trimmed = arguments.trim();
    if trimmed.is_empty() { return "{}".into(); }         if trimmed.is_empty() { return json!({}); }
    if serde_json::from_str::<Value>(trimmed).is_ok()     serde_json::from_str(trimmed)
        { input } else { "{}".into() } }                      .unwrap_or_else(|_| json!({})) }
```
- **Recommendation:** Add a shared `provider/infrastructure` helper — e.g. `tool_args::sanitized_object(raw: &str) -> Value` returning the parsed object or `{}` — and derive the `String` form from it where a string is needed. The "never let a garbled turn poison the next request" rule then has one owner. Do NOT implement — scan-only.

## Strengths
- **Cross-adapter error classification is already centralized:** `provider/infrastructure/http_error.rs::error_from_status` is shared by both chat adapters and the embeddings adapter, with its 429/408-are-transient nuance written once — exactly the pattern the other findings recommend.
- **The tool layer shares its scaffolding well:** `tools/infrastructure/args` (`parse_args` → the uniform `invalid arguments` outcome), `tools/application/tool` (`function_schema`/`confirm`/`simple_command`), and `tools/infrastructure/support` (`stat_guard`, `ensure_parent_dirs`, capped read/search) keep the ten fs tools thin and consistent — no per-tool re-implementation of parsing or path rendering.
- **Single-source auth rules:** `ProviderKind::requires_api_key` is deliberately the one rule shared by the `/provider` wizard and the factory (commented as such), preventing the wizard and the adapter selector from drifting on which kinds may be keyless.
