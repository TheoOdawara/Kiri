# Kiri — Project Working Contract

Async Rust CLI agent harness that talks to NVIDIA's OpenAI-compatible chat API (streaming REPL,
tool-calling with a filesystem sandbox, per-call approval).
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
- **Modules (bounded contexts):** `agent` (conversation domain + the `RunTurn` agent loop + the UI ports
  `Presenter`/`ApprovalPolicy`/`AgentIo`), `provider` (the `CompletionProvider` port + the OpenAI-compatible
  adapter: wire DTOs, SSE assembly), `tools` (the `Tool` trait + `ToolRegistry` + the sandbox + one fs adapter
  per tool), `repl` (the `Terminal` + the REPL driving adapter). Planned: `session` (SQLite-persisted
  conversations).
- **shared/kernel:** cross-cutting primitives — `ToolCall`/`FunctionCall`, `AgentError` (thiserror).
  **shared/infra:** `config` (CLI + env + `Settings`).

**Invariants:** network I/O only in `provider/infrastructure`; filesystem I/O only in `tools/infrastructure`
(the sandbox is the single path chokepoint); `domain` has no I/O; the engine never touches stdin/stdout
directly (all UI via the `AgentIo` port). Ports return `AgentError`; `anyhow` only at the binary edge.

**Extending:** a new tool = one file under `tools/infrastructure/fs/` implementing `Tool`, registered in
`default_fs_tools`; a new provider = one adapter implementing `CompletionProvider`, chosen in `app::wire`.

Provider target: **NVIDIA**'s OpenAI-compatible endpoint `<base-url>/chat/completions`. The base URL is
injected via `Settings` into the provider adapter at `app::wire` (default
`https://integrate.api.nvidia.com/v1`); a future multi-provider feature externalizes it to a config file
(ref: `docs/decisions/0001-openai-compatible-provider.md`). The **model** and **API key** are read from the
environment, both required — `NVIDIA_MODEL` and `NVIDIA_API_KEY` (loaded from `.env` via `dotenvy`); the key
is **never** a CLI flag. The bearer header is always sent. See `docs/ollama.ps1` for the raw
OpenAI-compatible protocol shape.

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
