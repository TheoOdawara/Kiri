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
  **keyring** — OS credential store (Keychain / Cred Manager / Secret Service). **zeroize** — secret memory.
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

- **Layers, depending inward:** `domain/` = pure data/rules, no I/O · `application/` = use-cases + the
  **ports** they need, as **traits** (named by capability, no `I` prefix) · `infrastructure/` = **adapters**
  implementing the ports.
- **Modules (bounded contexts):** `agent` (the `AgentLoop` + the UI
  ports `Presenter`/`ApprovalPolicy`, plus the provider's `EventSink`; the conversation types it drives
  live in shared/kernel), `provider` (the `CompletionProvider`
  port + two API-key adapters — `openai` (chat-completions: NVIDIA / compatible / custom / OpenAI) and
  `anthropic` (Messages API) — plus the `SecretStore` port with `keyring`/`0600`-file adapters and the
  `factory` that picks the adapter from `(kind, auth)`; see ADRs 0011/0012), `tools` (the `Tool` trait + `ToolRegistry`
  + the `Sandbox` port — `FsSandbox` the fs adapter — + one fs adapter per tool), `tui` (the Elm-style `Model`/`update`/keymap + the `Bridge`
  adapter + the ratatui runtime — the sole front-end), `memory` (durable knowledge: `MemoryEntry`/`MemoryKind`
  domain — kinds include `preference` — + the `MemoryPort`/`ProjectStore`/`SharedStore` ports + adapters —
  `FileProjectMemory` for project memory in `<workspace>/.kiri/memory/`, `SqliteSharedMemory` for shared
  memory in `~/.kiri/memory/shared.db`, `DocsLibrary` over `docs/` — the `recall_memory`/`remember`/
  `consult_docs` tools, semantic recall via the `EmbeddingProvider` port with a keyword fallback (ADR 0014),
  and the end-of-session `Distiller` that learns automatically (ADR 0013); see ADRs 0010/0013/0014),
  `session` (SQLite-persisted conversations in `~/.kiri/sessions.db`, keyed by project: the `Session`
  domain + `SessionStore` port + `SqliteSessionStore`, driving `/resume` and `/sessions`; ADR 0013),
  `sync` (portable-profile sync to a private git repo: the `Git` port + `GitCli` + NDJSON export/merge +
  `SyncService`, behind `kiri sync …` and `/sync`; ADR 0015). Planned: a memory-management GUI.
- **shared/kernel:** cross-cutting primitives — `ToolCall`/`FunctionCall`, `AgentError` (thiserror),
  `ApprovalMode`, the conversation types (`Message`/`Role`/`StreamEvent`/`CompletedTurn`/`Conversation`,
  the shared data between `agent` and `provider` — their home here is what breaks the agent↔provider cycle),
  and the provider primitives (`ProviderKind`/`AuthMethod`/`Effort`/`ProviderProfile`/`Credential`/`Secret`),
  shared across `agent`, `provider`, `session`, `memory`, `config`, and `tui`. **shared/infra:** `config`
  (layered TOML + env + CLI → `Settings`, plus the global-config writers).

**Invariants:** network I/O only in `provider/infrastructure` — **except** `sync/infrastructure`, which
shells out to `git` to reach the user's profile repo (ADR 0015); filesystem I/O only in
`tools/infrastructure` (the `FsSandbox` adapter — behind the `tools/application::Sandbox` port — is the
single path chokepoint) — **except** the `memory`, `session`,
and `sync` contexts, which own their data dirs (`.kiri/memory`, `~/.kiri/memory`, `~/.kiri/sessions.db`,
`~/.kiri/sync`), plus `provider/infrastructure/secrets` (the keyring/`0600` credentials file) and
`shared/infra/config` (the `~/.kiri/config.toml` + dir creation) — all do their own file/SQLite I/O for
harness-owned storage, never for agent-supplied paths (ref ADRs 0010/0013/0015); `domain` has no I/O;
the engine never touches stdin/stdout
directly (all UI via the engine ports). Ports return `AgentError`; `anyhow` only at the binary edge.

**Extending:** a new tool = one file under `tools/infrastructure/fs/` implementing `Tool` (it receives
the `Sandbox` port as `&dyn Sandbox`), registered in `default_fs_tools`; a new provider = one adapter implementing `CompletionProvider` + a `(kind, auth)` arm in
`provider/infrastructure/factory`; a new memory/docs tool = one file under `memory/infrastructure/tools/`,
registered in `default_memory_tools`.

**Providers & config (ADRs 0011/0012):** provider-agnostic, **API key only** — subscription OAuth (Claude
Pro/Max, ChatGPT Plus/Pro) is intentionally **unsupported** (it requires impersonating the vendor's client,
which is ToS-banned and bans the user's account; `AuthMethod::Oauth` is modeled but non-wired). The provider
catalog, active selection, model lists, and effort live in **layered TOML** (`~/.kiri/config.toml` global ←
`<workspace>/.kiri/config.toml` project); **secrets** live in the OS keyring (or a `0600` file), keyed by
provider id, never in TOML. **No `.env`.** The untrusted project layer contributes **only `effort`** —
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
- **clap is the convention** — record any deviation as an ADR (`docs/decisions/`).
- Keep the provider base URL / model / key configurable; never hardcode.
- A PostToolUse hook auto-runs `cargo fmt` (+ clippy feedback) on `.rs` edits — see `docs/claude-tooling.md`.
- Recommended Claude Code tooling (MCPs/skills): see `docs/claude-tooling.md`.
