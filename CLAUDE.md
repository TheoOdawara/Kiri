# Kiri — Project Working Contract

Async Rust CLI agent harness that talks to NVIDIA's OpenAI-compatible chat API (streaming TUI,
tool-calling with a filesystem sandbox, approval modes: default/auto/plan).
Layers on the global contract — only project-specific rules below; when they conflict, this file wins.

## Stack

- Rust **1.96 stable**, **edition 2024** (intentional — stabilized in 1.85, valid on this toolchain).
- `tokio` (full) — async runtime. `reqwest` (json) — HTTP to the provider. `serde` + `serde_json` — payloads.
- **clap** (derive) — CLI parsing. **async-trait** — dyn-compatible async ports. **thiserror** — the typed
  `AgentError` kernel type. **anyhow** — error glue at the binary edge. **dotenvy** — `.env` loading.
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
- **Modules (bounded contexts):** `agent` (conversation domain + the `AgentLoop` + `ApprovalMode` + the UI
  ports `Presenter`/`ApprovalPolicy`, plus the provider's `EventSink`), `provider` (the `CompletionProvider`
  port + the OpenAI-compatible adapter: wire DTOs, SSE assembly), `tools` (the `Tool` trait + `ToolRegistry`
  + the sandbox + one fs adapter per tool), `tui` (the Elm-style `Model`/`update`/keymap + the `Bridge`
  adapter + the ratatui runtime — the sole front-end), `memory` (durable knowledge: `MemoryEntry`/`MemoryKind`
  domain + the `MemoryPort`/`ProjectStore`/`SharedStore` ports + adapters — `FileProjectMemory` for project
  memory in `<workspace>/.kiri/memory/`, `SqliteSharedMemory` for shared memory in `~/.kiri/memory/shared.db`,
  `DocsLibrary` over `docs/` — and the `recall_memory`/`remember`/`consult_docs` tools; see
  `docs/decisions/0010-memory-and-docs-knowledge.md`). Planned: `session` (SQLite-persisted conversations),
  and a memory-management GUI.
- **shared/kernel:** cross-cutting primitives — `ToolCall`/`FunctionCall`, `AgentError` (thiserror).
  **shared/infra:** `config` (CLI + env + `Settings`).

**Invariants:** network I/O only in `provider/infrastructure`; filesystem I/O only in `tools/infrastructure`
(the sandbox is the single path chokepoint) — **except** the `memory` context, which owns its data dirs
(`.kiri/memory`, `~/.kiri/memory`) and does its own file/SQLite I/O for harness-owned storage, never for
agent-supplied paths (ref ADR 0010); `domain` has no I/O; the engine never touches stdin/stdout
directly (all UI via the engine ports). Ports return `AgentError`; `anyhow` only at the binary edge.

**Extending:** a new tool = one file under `tools/infrastructure/fs/` implementing `Tool`, registered in
`default_fs_tools`; a new provider = one adapter implementing `CompletionProvider`, chosen in `app::wire`;
a new memory/docs tool = one file under `memory/infrastructure/tools/`, registered in `default_memory_tools`.

Provider target: **NVIDIA**'s OpenAI-compatible endpoint `<base-url>/chat/completions`. The base URL is
injected via `Settings` into the provider adapter at `app::wire` (default
`https://integrate.api.nvidia.com/v1`); a future multi-provider feature externalizes it to a config file
(ref: `docs/decisions/0001-openai-compatible-provider.md`). The **model** and **API key** are read from the
environment, both required — `NVIDIA_MODEL` and `NVIDIA_API_KEY` (loaded from `.env` via `dotenvy`); the key
is **never** a CLI flag. The bearer header is always sent. See `docs/ollama.ps1` for the raw
OpenAI-compatible protocol shape.

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

- Protected: **`main`** — never commit directly. No remote yet.
- Real work on feature branches (`feat/...`, `fix/...`).

## Language

- Chat: **Portuguese (pt-BR)**. Docs, CLAUDE.md, comments, identifiers: **English**.

## Project rules / gotchas

- **Code-only style:** clear names, minimal comments (why, not what), no teaching prose.
- **clap is the convention** — record any deviation as an ADR (`docs/decisions/`).
- Keep the provider base URL / model / key configurable; never hardcode.
- A PostToolUse hook auto-runs `cargo fmt` (+ clippy feedback) on `.rs` edits — see `docs/claude-tooling.md`.
- Recommended Claude Code tooling (MCPs/skills): see `docs/claude-tooling.md`.
