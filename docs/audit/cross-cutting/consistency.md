# Audit — Naming & Convention Consistency (cross-cutting)

> Scope: the whole `src/` tree (read in full across every module: `shared/kernel`, `shared/infra`, `agent`, `provider`, `tools`, `tui`, `memory`, `session`, `sync`, plus `main.rs`/`app.rs`/`modules.rs`/`characterization.rs`). This pass owns only what the whole-graph view reveals — module-file convention, trait/error/DTO naming patterns, doc-comment presence, language mixing, and import/binding style across sibling files.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The codebase is, file-for-file, of high quality: names are descriptive, comments explain *why*, traits are capability-named with no `I` prefix, and `new`/`with_*` constructors follow the idiomatic base-vs-builder split consistently. The inconsistencies are **structural and cross-cutting** — patterns that each module applies internally-consistently but that diverge between modules. The headline issue is a clean split of the module-file convention: `memory`/`session`/`sync` use `mod.rs`, every other module uses `<name>.rs`. Three further whole-tree divergences track the same module fault line: the `#[async_trait]` macro form, a local `type Result<T>` alias, and abbreviated error-map helpers. None are correctness or security defects; all are maintainability/clarity nits that a single mechanical decision per item would resolve.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 0 | 5 | 4 |

## Findings

### [CONS-01] Unify the module-file convention: `mod.rs` vs `<name>.rs` are mixed across the tree
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** `mod.rs` style — `src/modules/memory/application/mod.rs`, `src/modules/memory/domain/mod.rs`, `src/modules/memory/infrastructure/mod.rs`, `src/modules/memory/infrastructure/tools/mod.rs`, `src/modules/session/application/mod.rs`, `src/modules/session/domain/mod.rs`, `src/modules/session/infrastructure/mod.rs`, `src/modules/sync/application/mod.rs`, `src/modules/sync/domain/mod.rs`, `src/modules/sync/infrastructure/mod.rs`. `<name>.rs` style — `src/modules/agent.rs`, `src/modules/agent/application.rs`, `src/modules/provider.rs`, `src/modules/provider/application.rs`, `src/modules/provider/infrastructure.rs`, `src/modules/provider/infrastructure/openai.rs`, `src/modules/provider/infrastructure/anthropic.rs`, `src/modules/tools.rs`, `src/modules/tools/application.rs`, `src/modules/tools/infrastructure.rs`, `src/modules/tui.rs`, `src/modules/tui/application.rs`, `src/modules/tui/domain.rs`, `src/modules/tui/infrastructure.rs`, `src/modules/tui/infrastructure/widgets.rs`, `src/shared/kernel.rs`, `src/shared/infra.rs` (and ~15 more).
- **Problem:** The two newest module clusters (`memory`/`session`/`sync` — 10 directories) declare their submodules from a `mod.rs` inside the directory, while every older module (`agent`/`provider`/`tools`/`tui`/`shared` — ~22 sibling files) declares them from a `<name>.rs` beside the directory. This is the single most visible cross-cutting inconsistency: a reader navigating the tree cannot predict where a module's submodule list lives, and search/jump tooling behaves differently per module. The two styles are functionally identical, so the divergence is pure drift.
- **Evidence:**
```rust
// src/modules/memory.rs  — declares the dir from the sibling file…
pub mod application;
pub mod domain;
pub mod infrastructure;
// …but src/modules/memory/application/mod.rs declares its children from inside the dir:
// pub mod distill;  pub mod memory_port;  pub mod project_memory; …
// Meanwhile src/modules/tui/domain.rs (sibling-file style) declares:
// pub mod command_menu;  pub mod model;  pub mod transcript;  pub mod view_state;
```
- **Recommendation:** Pick one convention tree-wide. Edition 2024 (and rustfmt guidance) prefer `<name>.rs`, which is also the majority here — convert the 10 `mod.rs` files in `memory`/`session`/`sync` to `<name>.rs` siblings. Do NOT implement now; this is scan-only.

### [CONS-02] Three different `#[async_trait]` macro spellings for the same intent
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** path-`?Send` form (29 sites) e.g. `src/modules/provider/application/completion_provider.rs:24`, `src/modules/tools/application/tool.rs:78`, `src/modules/agent/application/approval_policy.rs:27`; imported-bare form (14 files) e.g. `src/modules/memory/application/memory_port.rs:51`, `src/modules/session/application/session_store.rs:13`, `src/modules/sync/application/git.rs:17`; path-Send form (2 files) `src/modules/provider/application/embedding_provider.rs:7`, `src/modules/provider/infrastructure/openai/embeddings.rs:72`.
- **Problem:** Three spellings coexist: `#[async_trait::async_trait(?Send)]` (fully-qualified, single-threaded engine path), `use async_trait::async_trait;` + bare `#[async_trait]` (the `memory`/`session`/`sync` cluster), and `#[async_trait::async_trait]` (fully-qualified, `Send`). The `?Send` vs `Send` distinction is a *real* semantic choice (the streaming engine ports are `?Send`; the memory/embedding ports are `Send`) and is fine. But the **import-vs-path** split for the *same* `Send` semantics is pure style drift — `provider::embedding_provider` writes `#[async_trait::async_trait]` while `memory::memory_port` writes the imported `#[async_trait]` for identical `Send` traits.
- **Evidence:**
```rust
// src/modules/provider/application/embedding_provider.rs
#[async_trait::async_trait]            // path form, Send
pub trait EmbeddingProvider: Send + Sync { … }

// src/modules/memory/application/memory_port.rs
use async_trait::async_trait;
#[async_trait]                          // imported form, Send — same semantics, different spelling
pub trait MemoryPort: Send + Sync { … }
```
- **Recommendation:** Standardize on one path-qualified spelling per semantics — `#[async_trait::async_trait]` for `Send` and `#[async_trait::async_trait(?Send)]` for `?Send` — and drop the `use async_trait::async_trait;` imports, so the `?Send`/`Send` choice is always visible at the attribute and the import style never varies.

### [CONS-03] Local `type Result<T> = …AgentError` alias used in only three modules (and inconsistently within one)
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** present in 15 files, all under `memory`/`session`/`sync` — e.g. `src/modules/memory/application/memory_port.rs:11`, `src/modules/memory/infrastructure/sqlite_shared_memory.rs:13`, `src/modules/session/application/session_store.rs:7`, `src/modules/sync/application/sync_service.rs:13`, `src/modules/sync/infrastructure/memory_ndjson.rs:10`. Absent everywhere in `agent`/`provider`/`tools`/`tui`/`shared`, which spell `Result<…, AgentError>` in full. Diverges even *within* `sync`: `src/modules/sync/application/git.rs:21` writes `Result<GitOutput, AgentError>` in full while `src/modules/sync/application/sync_service.rs` aliases.
- **Problem:** The same fallible signature is written two ways depending on which module a reader is in, and the `sync` module itself does both. The alias shadows `std::result::Result` locally, so a reader skimming a `memory` file sees a bare `Result<T>` whose error type is implicit, whereas the provider/agent code states `AgentError` at every signature.
- **Evidence:**
```rust
// src/modules/sync/application/git.rs  (no alias)
async fn run(&self, args: &[&str], cwd: &Path) -> Result<GitOutput, AgentError>;
// src/modules/sync/application/sync_service.rs  (alias — same module!)
type Result<T> = std::result::Result<T, AgentError>;
pub async fn push(&self) -> Result<String> { … }
```
- **Recommendation:** Decide one policy tree-wide — either adopt the `type Result<T> = Result<T, AgentError>` alias everywhere (and document it) or remove it from the three modules and spell `AgentError` in full to match the majority. Whichever is chosen, make `sync` internally consistent.

### [CONS-04] `MemoryPort` is the only `Port`-suffixed trait; `MemoryPortImpl` is the only `*Impl` struct
- **Severity:** Low
- **Category:** naming
- **Location:** `src/modules/memory/application/memory_port.rs:52` (`pub trait MemoryPort`), `src/modules/memory/application/memory_port.rs:75` (`pub struct MemoryPortImpl<P, S>`).
- **Problem:** Across the 17 application/infra traits the capability-named, suffix-free convention is followed everywhere — `CompletionProvider`, `EmbeddingProvider`, `SecretStore`, `Sandbox`, `CommandSandbox`, `Presenter`, `ApprovalPolicy`, `ToolObserver`, `EventSink`, `Tool`, `ProjectStore`, `SharedStore`, `ProjectMemory`, `SharedMemory`, `SessionStore`, `Git` — except `MemoryPort`, which alone carries a `Port` suffix (the very word the other ports' doc-comments use to *describe* themselves, e.g. `sandbox.rs:18` "Port: …"). Its default implementation `MemoryPortImpl` is likewise the only `*Impl`-suffixed type in the codebase (a Java-ism; every other adapter is named by what it is — `FileProjectStore`, `SqliteSharedStore`, `OpenAiProvider`, `UnconfiguredProvider`).
- **Evidence:**
```rust
// src/modules/memory/application/memory_port.rs
pub trait MemoryPort: Send + Sync { … }     // lone "Port" suffix
pub struct MemoryPortImpl<P, S> { … }        // lone "*Impl" suffix
```
- **Recommendation:** Rename the trait to a capability noun (e.g. `Memory` or `MemoryAccess`) to match the other ports, and the struct to a descriptive adapter name (e.g. `HybridMemory` / `LayeredMemory`) rather than `…Impl`. Scan-only — do not implement.

### [CONS-05] The domain↔wire "message DTO" types are named three different ways; the `Dto` suffix is applied inconsistently
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** `src/modules/provider/infrastructure/openai/message_dto.rs:16` (`MessageDto`, plus `ContentDto`/`ToolCallDto`/`FunctionCallDto`), `src/modules/provider/infrastructure/anthropic/message_dto.rs:17` (`AnthropicMessage`, plus `ContentBlock`/`ImageSource` — no suffix), `src/modules/session/infrastructure/message_dto.rs:11` (`StoredMessage` — no suffix). Sibling wire files compound it: `src/modules/provider/infrastructure/openai/wire.rs` uses no suffix (`ChatRequest`, `Delta`, `StreamChoice`, `ToolCallFragment`) while `src/modules/provider/infrastructure/anthropic/wire.rs` uses the suffix throughout (`StreamEventDto`, `BlockDeltaDto`, `ContentBlockStartDto`, `MessageDeltaDto`, `ApiErrorDto`).
- **Problem:** All three modules solve the identical problem — a serde mirror of the domain `Message`, kept out of the domain — yet name the type `MessageDto`, `AnthropicMessage`, and `StoredMessage` respectively, all living in a file literally named `message_dto.rs`. Worse, the `Dto` suffix is applied in opposite directions between the two provider adapters: openai puts `Dto` on the message types and *omits* it on the wire types, while anthropic does exactly the reverse. A reader cannot infer from a type name whether it is a wire DTO.
- **Evidence:**
```rust
// openai/message_dto.rs   →  MessageDto, ContentDto, ToolCallDto   (Dto suffix)
// openai/wire.rs          →  ChatRequest, Delta, StreamChoice      (NO suffix)
// anthropic/message_dto.rs→  AnthropicMessage, ContentBlock        (NO suffix)
// anthropic/wire.rs       →  StreamEventDto, BlockDeltaDto, ApiErrorDto (Dto suffix)
// session/infrastructure/message_dto.rs → StoredMessage            (NO suffix)
```
- **Recommendation:** Choose one rule — e.g. "wire/DTO types carry no `Dto` suffix; the module path conveys the layer" (the dominant idiom in `openai/wire.rs`) — and apply it across both provider adapters and the session store, so `MessageDto`/`StreamEventDto`/`BlockDeltaDto`/`MessageDeltaDto`/`ApiErrorDto`/`ContentBlockStartDto` lose the suffix and the three "message mirror" types converge on one naming shape.

### [CONS-06] Four different naming shapes for the same enum↔wire-string conversion; `MemoryKind::from_str` shadows `FromStr`
- **Severity:** Low
- **Category:** naming
- **Location:** `src/shared/kernel/provider.rs:81` (`AuthMethod::as_wire`), `src/modules/provider/infrastructure/openai/message_dto.rs:105` (free fn `wire_role`), `src/modules/session/infrastructure/message_dto.rs:23` & `:34` (free fns `role_to_str`/`role_from_str`), `src/modules/memory/domain/entry.rs:41` & `:53` (methods `MemoryKind::as_str`/`from_str`).
- **Problem:** "Map an enum to/from its wire string" is implemented four ways with four naming schemes: a private method `as_wire`, a free `const fn wire_role`, a free-function pair `role_to_str`/`role_from_str`, and a method pair `as_str`/`from_str`. Two of them (`wire_role` and `role_to_str`) even encode the *same* `Role` → `"system"/"user"/…` table in two places. Separately, `MemoryKind::from_str(&str) -> Option<Self>` is an inherent method with the exact name and shape of the `std::str::FromStr` trait method (clippy's `should_implement_trait` family), while `Display` *is* implemented on the same type — so the conversion API is half-trait, half-inherent.
- **Evidence:**
```rust
// src/modules/memory/domain/entry.rs
pub fn as_str(&self) -> &'static str { … }     // pairs with…
pub fn from_str(s: &str) -> Option<Self> { … } // …an inherent from_str (not impl FromStr)
impl std::fmt::Display for MemoryKind { … }     // but Display IS a trait impl
```
- **Recommendation:** Settle on one shape for enum↔wire conversions (e.g. an `as_wire(&self) -> &str` method + an `impl FromStr`/`impl Display`), and have `Role` expose it once so `wire_role` and `role_to_str` stop re-encoding the same table. Rename `MemoryKind::from_str` (e.g. `parse`/`from_wire`) or implement `FromStr` properly.

### [CONS-07] Closure error binding and error-map helpers use terse abbreviations in some files, descriptive names in others
- **Severity:** Low
- **Category:** naming
- **Location:** terse `map_err(|e| …)` in 5 files — `src/shared/infra/config.rs`, `src/modules/tools/infrastructure/sensitive.rs`, `src/modules/tui/infrastructure/clipboard.rs`, `src/modules/provider/infrastructure/secrets/keyring_store.rs`, `src/modules/provider/infrastructure/secrets/file_store.rs`; descriptive `map_err(|error| …)` in 10 files — e.g. `src/modules/provider/infrastructure/openai/provider.rs`, `src/modules/tools/infrastructure/exec.rs`, `src/modules/sync/infrastructure/git_cli.rs`, `src/modules/session/infrastructure/sqlite_session_store.rs`. Abbreviated error-map helper fns: `mem` (`src/modules/memory/infrastructure/file_project_memory.rs:14`, `src/modules/memory/infrastructure/sqlite_shared_memory.rs:16`), `sess` (`src/modules/session/infrastructure/sqlite_session_store.rs:20`), `ser` (`src/modules/sync/infrastructure/memory_ndjson.rs:25`).
- **Problem:** The project contract mandates self-documenting names, yet the error-binding identifier splits two-to-one between `|error|` and `|e|` across files, and three modules each define a one-word abbreviated helper (`mem`/`sess`/`ser`) for "map this failure into the kernel error variant" — three different abbreviations for the same idea, none self-describing (`ser` in `memory_ndjson` even maps to `AgentError::Memory`, not "serialize").
- **Evidence:**
```rust
// src/modules/provider/infrastructure/secrets/keyring_store.rs   (terse)
.map_err(|e| AgentError::Secret(format!("keyring read: {e}")))
// src/modules/provider/infrastructure/openai/provider.rs         (descriptive)
.map_err(|error| AgentError::Provider(format!("failed to reach provider: {error}")))
// src/modules/sync/infrastructure/memory_ndjson.rs
fn ser<E: std::fmt::Display>(error: E) -> AgentError { AgentError::Memory(error.to_string()) }
```
- **Recommendation:** Standardize the closure binding on `|error|` tree-wide, and give the `mem`/`sess`/`ser` helpers a shared, descriptive name (e.g. `memory_error`/`session_error`, or a single generic mapper) so the same operation reads the same in every module.

### [CONS-08] Module-root doc-comment presence is inconsistent across re-export roots
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/provider/infrastructure/anthropic.rs:1` carries a `//!` module doc; its sibling `src/modules/provider/infrastructure/openai.rs:1` and every other `<dir>.rs` re-export root (`src/modules/agent.rs`, `src/modules/tools.rs`, `src/modules/tui.rs`, `src/modules/memory.rs`, `src/modules/tui/domain.rs`, `src/shared/kernel.rs`, …) open straight with `pub mod …` and no module doc.
- **Problem:** `anthropic.rs` is the lone re-export root that documents the context with a `//!` header; the parallel `openai.rs` adapter root has no such doc despite being the same kind of file. Doc-comment presence on equivalent files should be uniform — either every adapter/context root gets a one-line `//!` describing the context, or none do (the prose lives in `provider/infrastructure/<x>/provider.rs` for both today).
- **Evidence:**
```rust
// src/modules/provider/infrastructure/anthropic.rs
//! The Anthropic Messages API adapter (API key). Translates a domain `TurnRequest` …
pub mod message_dto;
// src/modules/provider/infrastructure/openai.rs   — no //! header, same kind of file
pub mod arguments;
```
- **Recommendation:** Decide a rule for re-export roots (recommended: keep them bare and let the substantive doc live on the concrete `provider.rs`/`adapter` types) and either remove the `//!` from `anthropic.rs` or add equivalent one-liners to the other context roots, so siblings match.

### [CONS-09] Model-facing tool-result strings mix English and Portuguese
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** Portuguese model-facing tool results in `src/modules/agent/application/agent_loop.rs:167-172` ("Plano apresentado ao usuário para aprovação.", "ignorada: present_plan encerra o turno"), `:203` ("ignorada: execução interrompida no checkpoint"), `:211` ("ignorada: sessão encerrada"), `:314` ("ignorada: interrompida pelo usuário"). English model-facing tool results in `src/modules/tools/application/tool.rs:19-25` (`ToolOutcome::into_message_content` → "declined by user", "error: …") and every fs/memory tool `execute` (e.g. `write_file.rs` "wrote {} bytes to {}", `agent_loop.rs:266` "'{}' is blocked in plan mode").
- **Problem:** These strings are the `content` of `role: tool` messages fed back into the conversation — read by the *model*, not shown to the user. The project's language rule is "chat pt-BR; code/identifiers English", and the *user-facing* confirmation prompts (e.g. `read_file.rs` "Ler o arquivo. Aprova executar…") are correctly pt-BR and are NOT flagged. But the *model-facing* tool results are split: `agent_loop` writes them in Portuguese while every actual tool writes them in English, so the same audience (the model) receives two languages within one turn's history. This is an inconsistency in which language model-facing protocol text uses.
- **Evidence:**
```rust
// src/modules/agent/application/agent_loop.rs  — model-facing tool result, pt-BR
conversation.push(Message::tool_result(call.id.as_str(), "ignorada: sessão encerrada".to_string()));
// src/modules/tools/application/tool.rs        — model-facing tool result, English
ToolOutcome::Declined => "declined by user".to_string(),
```
- **Recommendation:** Pick one language for model-facing tool-result/protocol strings (English is the natural choice, matching every tool and the contract's "code in English") and convert the `agent_loop` pt-BR results to match — leaving the genuinely user-facing pt-BR confirmation prompts untouched.

## Strengths
- **Trait discipline is excellent and uniform:** all 17 ports are capability-named with no `I` prefix, `domain` types are serde-free with wire mapping pushed to DTOs, and `?Send` vs `Send` on the async traits is a deliberate, correct distinction.
- **Constructor naming is idiomatic and consistent:** `new` for the base constructor and chained `with_*` builders (`MemoryPortImpl::with_embedder`, `Model::with_provider_catalog`, `FsSandbox::with_confinement`) are applied the same way everywhere — no `new` vs builder confusion.
- **Test-module convention is uniform:** every file uses `#[cfg(test)] mod tests { use super::*; … }` with descriptive `snake_case` test names, and secret-bearing types (`Secret`, `ProviderWizard`) consistently redact in `Debug` — a genuinely well-held cross-cutting invariant.
