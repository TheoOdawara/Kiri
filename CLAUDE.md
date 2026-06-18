# T-Cli — Project Working Contract

Async Rust CLI that talks to a local Ollama server (OpenAI-compatible chat API).
Layers on the global contract — only project-specific rules below; when they conflict, this file wins.

## Stack

- Rust **1.96 stable**, **edition 2024** (intentional — stabilized in 1.85, valid on this toolchain).
- `tokio` (full) — async runtime. `reqwest` (json) — HTTP to Ollama. `serde` + `serde_json` — payload models.
- **clap (derive)** — decided CLI-parsing convention (not yet added).
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

Layered, single binary. Dependencies point one way: `main → services → models`; `models` depends on nothing.

- `src/main.rs` — entry + `#[tokio::main]` + **clap** parsing + dispatch. Thin: parse → call a service →
  render to stdout. No business logic, no HTTP here.
- `src/models/` — plain data types (serde structs/enums) for Ollama request/response payloads.
  `models/chat.rs` = chat-completion types (messages, `Usage` token counts). Derive
  `Serialize`/`Deserialize`/`Debug`. No I/O, no logic.
- `src/services/` — external integrations. `services/ollama.rs` owns the Ollama HTTP client: build request →
  send via `reqwest` → deserialize into `models` → return `Result`. **All network I/O lives here**; never call
  `reqwest` outside `services/`.

Data flow: CLI args (clap) → `services::ollama` builds + sends → Ollama API → deserialize into `models::chat`
→ `main` renders. Errors propagate as `Result` with `?`; fail fast, never swallow.

Ollama target: endpoint `http://<host>:11434/v1/chat/completions`, OpenAI-compatible messages
(ref: `docs/ollama.ps1`). Host/model are **configurable (flag/env)** — never hardcode `192.168.0.240`.

## Branches

- Protected: **`main`** — never commit directly. No remote yet.
- Real work on feature branches (`feat/...`, `fix/...`).

## Language

- Chat: **Portuguese (pt-BR)**. Docs, CLAUDE.md, comments, identifiers: **English**.

## Project rules / gotchas

- **Code-only style:** clear names, minimal comments (why, not what), no teaching prose.
- **clap is the convention** — record any deviation as an ADR (`docs/decisions/`).
- Keep the Ollama host/model configurable; never hardcode.
- A PostToolUse hook auto-runs `cargo fmt` (+ clippy feedback) on `.rs` edits — see `docs/claude-tooling.md`.
- Recommended Claude Code tooling (MCPs/skills): see `docs/claude-tooling.md`.
