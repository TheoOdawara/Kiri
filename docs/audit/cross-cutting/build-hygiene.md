# Audit — Build hygiene: clippy, dead code, dependencies

> Scope: whole-crate compiler/linter output (`cargo clippy --all-targets`), every `#[allow(...)]` across `src/`, and `Cargo.toml` cross-checked against actual usage of each dependency + feature flag.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
Build hygiene is strong on the basics: `cargo clippy --all-targets` is **warning- and error-clean** (verified with a forced rebuild, exit 0; no rustc or clippy diagnostics on lib, bin, or test targets), `unsafe_code = "forbid"` is set crate-wide, and **every** declared dependency and feature flag is genuinely exercised — there is no truly unused crate. The real issues are below the compiler line: 21 `#[allow(dead_code)]`/`#[allow(clippy::too_many_arguments)]` suppressions that, taken as a whole-graph view, hide one genuinely-dead function, several test-only helpers that should be `#[cfg(test)]`-gated rather than allow-suppressed, a large speculative "future UI" API surface kept alive by allow attributes (including a duplicated double-port for memory persistence), a byte-for-byte duplicated `now_rfc3339()` helper, and a deprecated/unmaintained `serde_yaml` dependency. None are correctness or security-critical; all are maintainability/YAGNI debt.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 0 | 5 | 2 |

## Findings

### [BUILD-01] Inventory the `#[allow(dead_code)]` suppressions — one is fully dead, four are test-only mis-gated
- **Severity:** Medium
- **Category:** dead-code
- **Location:** `src/modules/memory/domain/entry.rs:132`, `src/modules/memory/domain/entry.rs:28`, `src/modules/memory/domain/entry.rs:125`, `src/modules/tools/application/registry.rs:47`, `src/modules/tools/infrastructure/sensitive.rs:67`
- **Problem:** Clippy is clean only because dead code is silenced, not removed. The whole-graph view of the 21 `#[allow]` sites separates three classes, and two of them are defects against the "nothing speculative / test scaffolding does not live in production code" bar:
  - **Genuinely dead (zero call sites, including tests):** `MemoryEntry::add_tags` — grep for `add_tags` returns only its definition. It is not reserved-for-anything in practice; it is unreachable code kept alive purely by the allow.
  - **Test-only helpers wearing `#[allow(dead_code)]` instead of `#[cfg(test)]`:** `MemoryKind::all` (used only at `entry.rs:170`, inside `#[cfg(test)] mod tests`), `MemoryEntry::update_content` (used only at `entry.rs:190` and `sqlite_shared_memory.rs:437`, both tests), `ToolRegistry::is_destructive` (used only by `registry.rs:205-214` tests), `SensitiveMatcher::empty` (every call site is a `#[cfg(test)]` helper). Marking production items `#[allow(dead_code)]` to satisfy a test is weaker than `#[cfg(test)]`: it ships the symbol in the real binary and mislabels test scaffolding as "reserved API."
- **Evidence:**
```rust
// entry.rs:131 — no caller anywhere in the tree
/// Add tags. Reserved for the future memory-management UI.
#[allow(dead_code)]
pub fn add_tags(&mut self, tags: impl IntoIterator<Item = String>) { ... }

// registry.rs:44 — "Currently unused in the engine path ... kept as a classification test assertion"
#[allow(dead_code)]
pub fn is_destructive(&self, name: &str) -> bool { ... }
```
- **Recommendation:** Delete `add_tags` (git preserves it for when the UI lands). Re-gate the four test-only items with `#[cfg(test)]` (or move them into their `mod tests`) so they leave the production binary and stop masquerading as reserved API. Do not implement here — scan-only.

### [BUILD-02] Speculative "future-UI" port surface retained behind allow attributes — including a duplicated double-port for memory
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/memory/application/project_memory.rs:12`, `src/modules/memory/application/shared_memory.rs:11`, `src/modules/memory/application/project_store.rs:18`, `src/modules/memory/application/shared_store.rs:18`, `src/modules/session/domain/session.rs:10`, `src/modules/provider/application/secret_store.rs:18`, `src/modules/session/application/session_store.rs:39`
- **Problem:** Two whole traits (`ProjectMemory`, `SharedMemory`) carry a trait-level `#[allow(dead_code)]`, and a further cluster of methods/fields (`ProjectStore`/`SharedStore::list_by_kind|list_by_tag|list_by_project`, `SecretStore::delete`, `SessionStore::delete`, `Session::{project_id, created_at, updated_at}`) are individually allow-suppressed. Grep confirms the `list_by_*` surface is invoked **only** by adapter delegation (`file_project_store.rs:36/40`, `sqlite_shared_store.rs:35/39/43`) and by tests — never by the agent runtime. This is a sizeable speculative API kept alive for an unbuilt "memory/session-management UI" (YAGNI). Worse, memory persistence is modeled as **two parallel port tiers** for the same concept: `ProjectMemory`/`SharedMemory` (full-CRUD "persistence port") and `ProjectStore`/`SharedStore` (the "use-cases" port the wiring actually consumes), with overlapping `save`/`search`/`list_by_*` methods — the Store adapter just delegates to the Memory port. One of the two tiers is redundant indirection.
- **Evidence:**
```rust
// project_memory.rs:9 — whole trait suppressed, "reserved for the future memory-management UI"
#[allow(dead_code)]
#[async_trait]
pub trait ProjectMemory: Send + Sync { /* init/save/load/delete/search/list/list_by_*/count */ }

// file_project_store.rs:35 — the only non-test caller of list_by_kind is a pass-through
async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>> {
    self.inner.list_by_kind(kind, limit).await
}
```
- **Recommendation:** Decide one of: (a) collapse the `Memory` + `Store` double-port into the single port the runtime actually wires, and delete the unused `list_by_*`/`delete`/`count` methods until a consumer exists; or (b) if the UI is genuinely imminent, track the surface in a tracking issue and keep one port only. Either way the trait-level `#[allow(dead_code)]` should not be the mechanism that keeps an entire unused contract compiling. Scan-only — flag, do not refactor.

### [BUILD-03] `now_rfc3339()` helper duplicated byte-for-byte across two modules
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/memory/domain/entry.rs:96`, `src/modules/session/infrastructure/sqlite_session_store.rs:26`
- **Problem:** The exact same function — identical body **and** identical doc comment (including the justification for the `.unwrap_or_default()`) — is defined independently in the memory domain and the session infrastructure. This is whole-graph duplication only visible by looking across modules: two homes for one primitive means two places to fix if the format or the error stance changes, and it duplicates the (correct) reasoning comment that justifies the swallowed format error.
- **Evidence:**
```rust
// entry.rs:96 AND sqlite_session_store.rs:26 — identical
/// RFC3339 timestamp for "now". Formatting a valid UTC instant cannot fail in practice; the empty
/// fallback keeps this runtime path total without an `unwrap` (forbidden outside tests).
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
```
- **Recommendation:** Promote one `now_rfc3339()` to a shared time helper (e.g. `shared/kernel`), since both the memory and session contexts depend on it. The `sync/domain/merge.rs` RFC3339 *parsing* is the symmetric counterpart and could share the same module. Scan-only.

### [BUILD-04] Four `#[allow(clippy::too_many_arguments)]` in runtime.rs suppress a real parameter-threading smell
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/tui/infrastructure/runtime.rs:260`, `src/modules/tui/infrastructure/runtime.rs:813`, `src/modules/tui/infrastructure/runtime.rs:1266`, `src/modules/tui/infrastructure/runtime.rs:1442`
- **Problem:** Four functions exceed clippy's argument threshold and silence the lint rather than addressing it: `Tui::new` (10 params), `drive_turn` (11 params), `open_session` (8 params), `drive_distillation` (8 params). The lint is firing on a genuine signal — these signatures thread the same cluster of runtime handles (`terminal`, `events`, `ticker`, `model`, `conversation`, session state) positionally through multiple call sites, which is error-prone (positional swaps compile) and hard to read.
- **Evidence:**
```rust
// runtime.rs:813
#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    agent_loop: &AgentLoop, conversation: &mut Conversation, sandbox: &dyn Sandbox,
    bridge: &mut Bridge, model: &mut Model, engine_rx: &mut mpsc::UnboundedReceiver<EngineMsg>,
    cancel: &CancelToken, pending_reply: &mut Option<oneshot::Sender<Approval>>,
    terminal: &mut DefaultTerminal, events: &mut EventStream, ticker: &mut Interval,
) -> Result<()> { ... }
```
- **Recommendation:** Bundle the stable co-travelling handles into a small context struct (e.g. a `LiveUi { terminal, events, ticker }` borrowed `&mut`, and/or a per-turn context holding `model`/`conversation`/session cursors) so each function drops to a handful of named arguments and the allow can be removed. Scan-only — propose, do not implement.

### [BUILD-05] `serde_yaml` dependency is unmaintained/deprecated (`0.9.34+deprecated`)
- **Severity:** Medium
- **Category:** security
- **Location:** `Cargo.toml:18` (`serde_yaml = "0.9"`), resolves in `Cargo.lock` to `0.9.34+deprecated`; sole use site `src/modules/memory/infrastructure/file_project_memory.rs:181` / `:191`
- **Problem:** `serde_yaml` was archived and deprecated by its author; the resolved version literally carries the `+deprecated` build metadata, and `cargo audit` flags it as unmaintained (RUSTSEC unmaintained advisory). It will receive no security or correctness fixes. It is pulled in for a single, narrow purpose — parsing/serializing the YAML front-matter of project-memory Markdown files — so the blast radius is small but the input (file front-matter) is attacker-influenceable if a repo ships crafted `.kiri/memory` files.
- **Evidence:**
```rust
// file_project_memory.rs:181
serde_yaml::from_str(fm).map_err(mem)?
// file_project_memory.rs:191
let front_matter = serde_yaml::to_string(entry).map_err(mem)?;
```
- **Recommendation:** Track as `security-debt`. Migrate the front-matter (de)serialization to a maintained crate (`serde_yaml_ng` / `serde_norway`) or, since the project already standardizes on TOML for config, consider TOML front-matter to drop the YAML dependency entirely. Scan-only.

### [BUILD-06] Four oversized files exceed a single responsibility (whole-graph size view)
- **Severity:** Low
- **Category:** file-size
- **Location:** `src/modules/tui/application/keymap.rs` (2274 LOC), `src/modules/tui/infrastructure/runtime.rs` (2072 LOC), `src/modules/agent/application/agent_loop.rs` (1431 LOC), `src/shared/infra/config.rs` (1054 LOC)
- **Problem:** From the portfolio view these four files are far larger than their siblings (the next-largest is `view_state.rs` at 767) and each clearly hosts multiple responsibilities — e.g. `runtime.rs` mixes terminal setup, the turn driver, session open/flush, and distillation; `config.rs` mixes layered-TOML loading, CLI arg parsing, and global-config writers. Large files raise merge friction and hide the kind of intra-file duplication seen in BUILD-03/BUILD-04.
- **Evidence:** `wc -l` over `src/` (top of the distribution):
```
2274 src/modules/tui/application/keymap.rs
2072 src/modules/tui/infrastructure/runtime.rs
1431 src/modules/agent/application/agent_loop.rs
1054 src/shared/infra/config.rs
```
- **Recommendation:** Defer concrete splits to the owning per-module passes (tui/agent/config); from the build-hygiene view, flag these four as the priority split candidates — e.g. `runtime.rs` → `terminal_setup` + `turn_driver` + `session_ops` + `distillation`; `config.rs` → `load` (layering) vs `cli` (clap) vs `writers`. Scan-only.

### [BUILD-07] tokio `net` feature is exercised only in test targets
- **Severity:** Low
- **Category:** dead-code
- **Location:** `Cargo.toml:29` (`tokio` features include `"net"`); only call sites `src/modules/provider/infrastructure/{anthropic,openai}/provider.rs` and `openai/embeddings.rs`, all inside `#[cfg(test)]` (`TcpListener::bind` for hang/timeout regression tests)
- **Problem:** Unlike every other declared tokio feature (`fs`, `process`, `time`, `sync`, `macros`, `io-util`, `rt-multi-thread`), `net` has **no** non-test consumer — the production HTTP path goes through `reqwest`, which carries its own transport. The feature is enabled in `[dependencies]`, so it is compiled into the release binary purely to satisfy the timeout regression tests.
- **Evidence:**
```rust
// openai/provider.rs:145 (inside #[cfg(test)]) — the only kind of net use
use tokio::net::TcpListener;
let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
```
- **Recommendation:** Acceptable as-is (tests live in the crate and cargo cannot cleanly per-feature-gate the same dep for dev-only), but note it explicitly — e.g. a one-line comment in `Cargo.toml` that `net` is test-only — so a future reader does not assume the runtime opens sockets directly. Scan-only.

## Strengths
- **Clippy/rustc are genuinely clean** across lib, bin, and all test targets (confirmed via a forced rebuild + `--all-targets`, exit 0, zero diagnostics) — the `[lints.clippy] all = warn` floor plus the `-D warnings` DoD gate is being honoured.
- **`unsafe_code = "forbid"`** is set crate-wide and uncircumvented (no `#[allow(unsafe_code)]` anywhere), and there are **no** debug-print leftovers in the engine — the only `println!`/`eprintln!` are at the binary edge (`main.rs`) and composition root/config diagnostics.
- **Dependency hygiene is tight:** every crate in `Cargo.toml` is used, and each declared feature flag maps to a real call (`uuid` v7 → `Uuid::now_v7`, `crossterm` event-stream → `EventStream`, `reqwest` stream → `bytes_stream`, `time` parsing → `OffsetDateTime::parse`, etc.) — no unused crate and no gratuitous feature beyond the test-only `net` noted above.
