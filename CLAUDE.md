# Kiri — Project Working Contract

Async Rust CLI agent harness, **provider-agnostic by API key** — NVIDIA (default), any OpenAI-compatible
/ custom endpoint, OpenAI (GPT), and Anthropic (Claude), switchable live from the TUI (streaming TUI,
tool-calling with a filesystem sandbox, approval modes: default/auto/plan).
Layers on the global contract — only project-specific rules below; when they conflict, this file wins.

## Stack

- Rust **1.96 stable**, **edition 2024** (intentional — stabilized in 1.85, valid on this toolchain).
- `tokio` (full) — async runtime. `reqwest` (json) — HTTP to the provider. `serde` + `serde_json` — payloads.
- **clap** (derive) — CLI parsing. **async-trait** — dyn-compatible async ports. **thiserror** — the typed
  `AgentError` kernel type. **anyhow** — error glue at the binary edge. **toml** — layered config.
  **dotenvy** — seed process env from the trusted `~/.kiri/.env` (never the cwd; ADR 0020). **zeroize** — secret memory.
- Single binary crate. No workspace, no lib target.

## Commands (verified on Rust 1.96)

- Format: `cargo fmt` · check: `cargo fmt --check`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Typecheck: `cargo check`
- Build: `cargo build` · release: `cargo build --release`
- Test: `cargo test`
- Run: `cargo run -- <args>`

Definition-of-done gate (overrides the global Biome/Jest defaults):
`cargo fmt --check → cargo clippy --all-targets -- -D warnings → cargo build → cargo test`, each exit 0.

## Architecture (enforced strictly)

**Modular hexagonal** (ports & adapters, vertical slices), single binary. Full rationale in
`docs/decisions/0003-modular-hexagonal-architecture.md`; it supersedes the old `main → services → models`
layering.

Layout: `src/main.rs` (~8-line entry) → `src/app.rs` (composition root, `wire`) + `src/shared/{kernel,infra}`
+ `src/modules/<context>/{domain,application,infrastructure}`.

`app::wire` (TUI) and `app::wire_sync` (headless `kiri sync`) are the **only** places adapters are chosen.
`wire` bundles the sync ports into a `SyncContext` for the runtime, with the shared store behind a
`SharedMemoryFactory` that opens it **lazily** on the first `/sync` (so a memory-off session that never
syncs creates no `shared.db`); boot outcomes (credential source, degraded stores) surface as `BootNotice`s
in the transcript rather than `eprintln!` behind the alternate-screen TUI.

- **Layers, depending inward:** `domain/` = pure data/rules, no I/O · `application/` = use-cases + the
  **ports** they need, as **traits** (named by capability, no `I` prefix) · `infrastructure/` = **adapters**
  implementing the ports.
- **Modules (bounded contexts):** `agent` (the `AgentLoop` + the UI
  ports `Presenter`/`ApprovalPolicy`/`ToolObserver`, plus the provider's `EventSink`; the conversation
  types it drives live in shared/kernel; `infrastructure` holds the one exception to "agent has no
  adapters" — the `task` tool (`TaskTool`), which dispatches a loaded `AgentProfile` as a nested, read-only
  `AgentLoop` turn behind `HeadlessIo`, structurally capped at depth 1; see ADR 0029), `provider` (the `CompletionProvider`
  port + two API-key adapters — `openai` (chat-completions: NVIDIA / compatible / custom / OpenAI) and
  `anthropic` (Messages API) — plus the `SecretStore` port with the `0600`-file adapter (`FileSecretStore`,
  the only backend — the OS keyring was removed, ADR 0020) and the
  `factory` that picks the adapter from `(kind, auth)`; see ADRs 0011/0012/0020), `tools` (the `Tool` trait + `ToolRegistry`
  + the `Sandbox` port — `FsSandbox` the fs adapter — + one fs adapter per tool, each doing native
  `std::fs` I/O on every platform; `run_command` is the sole shell-out surface (`sh -c` / `cmd /C`), see
  ADR 0018), `tui` (the Elm-style `Model`/`update`/keymap + the `Bridge`
  adapter + the ratatui runtime — the sole front-end), `memory` (durable knowledge: `MemoryEntry`/`MemoryKind`
  domain — kinds include `preference` — + the capability port `Memory` (impl by `LayeredMemory`, composing
  project + shared) over the use-case ports `MemoryStore`/`SharedStore` and the persistence ports
  `ProjectMemory`/`SharedMemory` + adapters —
  `FileProjectMemory` (a `ProjectMemory`) for project memory in `<workspace>/.kiri/memory/`,
  `SqliteSharedMemory` (a `SharedMemory`) for shared
  memory in `~/.kiri/memory/shared.db`, `DocsLibrary` over `docs/` — the `recall_memory`/`remember`/
  `consult_docs` tools, semantic recall via the `EmbeddingProvider` port with a keyword fallback (ADR 0014),
  the end-of-session `Distiller` that learns automatically (ADR 0013), and `recall_memory`'s cross-store
  dedup (`domain::similarity::is_near_duplicate`, shared with the distiller) dropping a shared hit that
  near-duplicates a project hit — project wins on provenance (ADR 0023); see ADRs 0010/0013/0014/0023),
  `session` (SQLite-persisted conversations in `~/.kiri/sessions.db`, keyed by project: the `Session`
  domain + `SessionStore` port + `SqliteSessionStore`, driving `/resume` and `/sessions`; ADR 0013),
  `sync` (portable-profile sync to a private git repo: the `Git` port + `GitCli` + NDJSON export/merge +
  `SyncService`, behind `kiri sync …` and `/sync`; ADR 0015), `extensions` (ADR 0021 workflow surface:
  rules/commands/agents/skills/hooks/mcp, each with a global `~/.kiri/` and project `<workspace>/.kiri/`
  layer, plus a third `Layer::Bundled` — Markdown compiled into the binary via `include_str!`
  (`infrastructure::bundled`), trusted like global, folded in as the lowest-precedence layer so a fresh
  install ships default skills (`plano`/`gh`/`commit`/`ponytail`) and read-only agent profiles
  (`search`/`planning`) with no `~/.kiri/` setup required; see ADR 0028 — the `Frontmatter` parser, the
  `Resource`/`Rule`/`CommandSpec`/`AgentProfile`/`Skill`/`Hook`/
  `McpServer` domain types, the `ExtensionsLoader` port + `FileExtensionsLoader` adapter assembling an
  `ExtensionCatalog`, the pure trust-gate decision `domain::gate::resolve`/`content_hash` (blake3) + the
  `0600`-file `ExtensionsTrustStore` recording TOFU approvals — `/rules`/`/commands`/`/agents`/`/skills`/
  `/hooks`/`/approve-hook`/`/mcp`/`/approve-mcp` manage it live), `hooks` (the sanctioned site for the
  `hooks` extension type's process I/O: the `HookRunner` port + `ShellHookRunner` adapter running a hook's
  command over the same confined shell-exec surface `run_command` uses; fire-and-forget — a run's outcome
  is a transcript notice, never a failure — dispatched at `SessionStart`/`SessionEnd`/`TurnEnd` via
  `tui::infrastructure::runtime::hook_dispatch`; `PreToolUse`/`PostToolUse` are discovered/gated but not yet
  dispatched, `ToolObserver`'s synchronous callbacks need new plumbing first), `mcp` (the sanctioned site
  for the `mcp` extension type's process/network I/O: the `McpConnection` port + `RmcpConnection` adapter —
  over the official `rmcp` SDK, the one third-party dependency this framework needed — spawning a server
  over stdio and completing the MCP handshake; `app::wire` connects every gate-approved server once at
  boot and wraps each discovered tool as an `McpToolProxy`, a real `Tool` registered into the same
  `ToolRegistry` as the built-in file tools, namespaced `mcp__<server_id>__<tool_name>`; stdio transport
  only, HTTP/SSE unstarted). Planned: a memory-management GUI.
- **shared/kernel:** cross-cutting primitives — `ToolCall`/`FunctionCall`, `AgentError` (thiserror),
  `ApprovalMode`, the conversation types (`Message`/`Role`/`StreamEvent`/`CompletedTurn`/`Conversation`,
  the shared data between `agent` and `provider` — their home here is what breaks the agent↔provider cycle),
  and the provider primitives (`ProviderKind`/`AuthMethod`/`Effort`/`ProviderProfile`/`Credential`/`Secret`),
  shared across `agent`, `provider`, `session`, `memory`, `config`, and `tui`. **shared/infra:** `config`
  (layered TOML + env + CLI → `Settings`, plus the global-config writers), `home` (cross-platform
  home-directory resolution — `$HOME` / `%USERPROFILE%` / `%HOMEDRIVE%%HOMEPATH%`, ADR 0018 — the single
  source `config` and `tools/application::path` both read).

**Invariants:** network I/O only in `provider/infrastructure` — **except** `sync/infrastructure`, which
shells out to `git` to reach the user's profile repo (ADR 0015); filesystem I/O only in
`tools/infrastructure` (the `FsSandbox` adapter — behind the `tools/application::Sandbox` port — is the
single path chokepoint) — **except** the `memory`, `session`,
`sync`, and `extensions` contexts, which own their data dirs (`.kiri/memory`, `~/.kiri/memory`,
`~/.kiri/sessions.db`, `~/.kiri/sync`, `.kiri/{rules,commands,agents,skills,hooks,mcp}`,
`~/.kiri/extensions_trust.json`), plus `provider/infrastructure/secrets` (the `0600` credentials file) and
`shared/infra/config` (the `~/.kiri/config.toml` + dir creation) — all do their own file/SQLite I/O for
harness-owned storage, never for agent-supplied paths (ref ADRs 0010/0013/0015/0021); process I/O for the
`hooks`/`mcp` extension types stays inside their own `infrastructure/` (`ShellHookRunner` routed through the
existing `tools/infrastructure::exec::run_shell` chokepoint; `RmcpConnection` over the `rmcp` SDK), each
guarded by its own architecture-guard test (`hooks_process_io_confined_to_infrastructure`,
`mcp_process_io_confined_to_infrastructure`) mirroring the domain-purity guard; `domain` has no I/O and
no UI-framework dependency — the **one** sanctioned exception is the TUI `InputBuffer` owning a
`tui_textarea::TextArea` (ADR 0017, guarded by a recursive domain-purity test); the engine never touches
stdin/stdout directly (all UI via the engine ports). Ports return `AgentError`; `anyhow` only at the binary edge.
These boundaries are not just convention: `src/architecture_guards.rs` holds `#[test]`s that walk `src/`
and fail the build if **domain purity** is re-breached — a `domain` file coupling to a UI crate
(ratatui/tui_textarea, only `InputBuffer` sanctioned, ADR 0017) or doing fs/net/db I/O. The inward
import-direction rule (application/domain must not import infrastructure) is enforced by convention and
review, not yet by a guard.

**Extending:** a new tool = one file under `tools/infrastructure/fs/` implementing `Tool` (it receives
the `Sandbox` port as `&dyn Sandbox`), registered in `default_fs_tools`; a new provider = one adapter implementing `CompletionProvider` + a `(kind, auth)` arm in
`provider/infrastructure/factory`; a new memory/docs tool = one file under `memory/infrastructure/tools/`,
registered in `default_memory_tools`.

**Providers & config (ADRs 0011/0012/0020):** provider-agnostic, **API key only** — subscription OAuth (Claude
Pro/Max, ChatGPT Plus/Pro) is intentionally **unsupported** (it requires impersonating the vendor's client,
which is ToS-banned and bans the user's account; `AuthMethod::Oauth` is modeled but non-wired). The provider
catalog, active selection, model lists, and effort live in **layered TOML** (`~/.kiri/config.toml` global ←
`<workspace>/.kiri/config.toml` project); **secrets** live in a `0600` `~/.kiri/credentials.json`, keyed by
provider id, never in TOML (the OS keyring was removed — ADR 0020). **`.env` only from `~/.kiri/.env`, never
the cwd** (`config::load_global_env` via `dotenvy::from_path`; ADR 0020): `~/.kiri/` is owner-only and
user-authored, so its `.env` carries global-layer trust and may seed API keys / env, while a hostile
project repo can never inject env — the argless cwd-reading `dotenvy::dotenv()` is build-failed by an
`architecture_guards` test. The untrusted project layer contributes **only `effort`** —
providers/sandbox/http/paths come from the trusted global layer (a malicious repo must not redirect a
credential or weaken the sandbox). `/provider` (switch + add wizard), `/models`, `/effort` manage it live.
A first run seeds NVIDIA and imports an API-key env var (`NVIDIA_API_KEY` / `KIRI_<ID>_API_KEY` …) once.
With **no env key and no stored credential**, the harness does not abort: it boots into a first-run
**onboarding** (a welcome wizard — provider list with NVIDIA preselected, then a masked API-key entry) and
gates prompt submission until a provider is saved. The composition root injects a null `CompletionProvider`
for that credential-less boot; the wizard's `SaveProvider` swaps in the real adapter.

## Error handling (production-ready, mandatory)

Robust error handling is non-negotiable on every surface — it is what keeps development fluid (a failure
that is surfaced is a failure you can fix; a swallowed one costs hours).

- **Nothing is swallowed.** Every fallible `Result`/`Option` is either **propagated** as a typed
  `AgentError` (`anyhow` only at the binary edge), **surfaced** to the user (a transcript `Notice`), or
  **deliberately ignored with a one-line comment justifying why it is safe**. A bare `let _ = <fallible>`
  (or `.ok()` / `.unwrap_or_default()` that hides a real failure) without that justification is a defect.
- **All I/O has a timeout.** Provider HTTP (`connect_timeout` + `read_timeout`), process exec (the
  kill-on-drop bound in `exec::run`), and any blocking await. A hung dependency must fail fast with a
  clear error — never hang silently. (Regression that motivated this: the provider client had no timeout,
  so the first message hung forever with no error.)
- **No silent no-ops on user intent.** An action that cannot run (busy, gated, invalid) gives visible
  feedback, never nothing.
- **No `unwrap`/`expect`/`panic!` on any runtime-reachable path** outside `#[cfg(test)]`; model the
  failure as `AgentError`.
- **Lifecycle state always resets.** Per-turn flags (e.g. `busy`) are reset on every exit path, including
  errors — a render/draw failure must not strand the UI.
- **Error paths are tested like behavior** — a feature's test contract includes its failure modes.

## Branches

- Protected: **`main`** — never commit directly. Remote: `origin` (github.com/TheoOdawara/Kiri).
- Real work on feature branches (`feat/...`, `fix/...`).

## Language

- Chat: **Portuguese (pt-BR)**. Docs, CLAUDE.md, comments, identifiers: **English**.

## Project rules / gotchas

- **Code-only style:** clear names, minimal comments (why, not what), no teaching prose.
- **Module convention:** declare submodules from the sibling `<name>.rs`, never an inner `mod.rs` (edition-2024 default, matches the whole tree).
- **`async_trait` spelling:** always path-qualified `#[async_trait::async_trait]`; add `(?Send)` only for the single-threaded engine ports (and any test double impl'ing one). Never the bare-import `use async_trait::async_trait;` + `#[async_trait]` form.
- **`Result` alias:** fallible port/adapter signatures use `AgentResult<T>` from `shared/kernel/error`; never a per-module `type Result<T>` shadow. `anyhow::Result` only at the binary edge.
- **enum ↔ wire shape:** one rule — `fn as_wire(&self) -> &'static str` (or a `const fn`) plus `impl std::str::FromStr` (or a clearly-named `from_wire`); never a half-trait/half-inherent `from_str` that shadows the trait.
- **DTO naming:** name a DTO for its role; no generic `Dto` suffix. Prefix `Wire`/`Stored` only where the bare name would collide with a kernel type.
- **Module-header `//!`:** a re-export root or a kernel module file carries **no** descriptive `//!` banner (per-item `///` docs cover the *what*); a `//!` is reserved for a non-obvious structural *WHY* (e.g. the single-layer `modules/agent.rs`).
- **Modal nav:** all four single-choice modals (menu/picker/wizard/approval/plan) wrap on Up/Down via the one `wrapping_step` helper.
- **clap is the convention** — record any deviation as an ADR (`docs/decisions/`).
- Keep the provider base URL / model / key configurable; never hardcode.
- A PostToolUse hook auto-runs `cargo fmt` (+ clippy feedback) on `.rs` edits (see `.claude/settings.json`).
