# Kiri — Repository Contract

Layers on the global contract. Product truth: `docs/requirements/`. Architecture decision:
`docs/decisions/0001-architecture.md`. Backlog: https://github.com/users/TheoOdawara/projects/7, in
one-week sprints.

## Stack

- Rust, `stable` channel (`rust-toolchain.toml`, with `rustfmt` and `clippy`); 1.98.1 installed at
  bootstrap. Edition 2024 (`rustfmt.toml`).
- A Cargo workspace per ADR 0001. No crate and no dependency exists yet; each dependency is chosen in the
  spec that first needs it.

## Language & scripts

- Application code: Rust.
- Repo scripts: Rust, as `cargo xtask`. No shell scripts.

## Conventions

Declared pending: stack-specific conventions are recorded here as the first crates land. The previous
codebase's conventions do not apply.

## Commands

| Gate | Invocation | Exit |
|---|---|---|
| Docs site build | `uvx zensical==0.0.67 build` | 0 |
| Docs site, served locally | `uvx zensical==0.0.67 serve --open` | — |

Declared pending: there is no `Cargo.toml`, so no code gate runs yet. Each one — static analysis, type
checking, formatting, build, tests — is recorded here, with its exact invocation and a validated exit
code, when the workspace lands. Until then `/closeout` runs the docs build only.

## Architecture

Decided in ADR 0001. Rules below apply from the first file written.

### Process model

- `kiri daemon` owns every session, for every provider. One daemon per user, started by the first
  client, exiting after an idle period with no running session and no client.
- Every client is an ACP client of the daemon: the TUI, `kiri -p`, and `kiri acp` (the IDE stdio
  bridge). No code path runs the engine inside a client process.
- Transport: Unix domain socket on Linux/macOS, named pipe on Windows, restricted to the owning user.

### Crates and imports

| Crate | Holds | May depend on | Public surface |
|---|---|---|---|
| `kiri-core` | Engine and session host | no other Kiri crate | The protocol only — ACP types plus `kiri/*` extension methods; everything else `pub(crate)` |
| `kiri-tui` | Inline interface (full-screen later) | `kiri-core`'s protocol | — |
| `kiri` | The binary: `daemon`, CLI, `-p`, `acp` | `kiri-core`, `kiri-tui` | — |

A new crate exists only when it has a consumer of its own.

Issue labels, one per crate: `area:core`, `area:tui`, `area:cli` (the `kiri` binary).

### Inside `kiri-core`

- Capability modules: `agent`, `provider`, `tools`, `permissions`, `sandbox`, `session`, `extensions`,
  `sync`. No `domain`/`application`/`infrastructure` layering.
- Modules call each other directly. Events exist only at the ACP boundary.
- A trait exists only with two or more real implementations: `Provider` and `Sandbox`.
- A new provider is one file in `provider/` plus its registration. A new OS sandbox is one file in
  `sandbox/`, selected at compile time.
- Streaming: SSE bytes → provider events → batches of about 10 ms → ACP events.
- Claude Pro/Max is an agent backend: the user's installed `claude` CLI in `-p` stream-json mode, its
  built-in tools disabled, Kiri's tools served to it through an MCP server, with a minimum CLI version
  check.
- A subagent is a child session in the daemon, on the same protocol.

### Extensions

- Plugins are data (Markdown skills, commands, agents) and child processes (hooks, MCP servers), in
  formats compatible with Claude Code's.
- Third-party code never runs inside the daemon process.
- Nothing from a plugin or a project activates without the user's trust approval.

### Persistence

Text only — JSONL, TOML, Markdown — under `~/.kiri` and `<project>/.kiri`. No SQLite.

## Branches

- `main`: releases only. Never commit directly.
- `staging`: integration. Every work branch is cut from `staging` and merged into it by PR. The end-to-end test of a release runs on `staging` before it merges into `main`.
- Work happens on `feat/…`, `fix/…`, `chore/…` branches. Remote: `origin` (github.com/TheoOdawara/Kiri).
- Neither `main` nor `staging` has GitHub branch protection configured. The rules above are convention
  only.

## Language

Chat: Portuguese (pt-BR). Docs, `AGENTS.md`, and identifiers: English.

## Gotchas

- `.claude/settings.json` runs `cargo fmt && cargo clippy --quiet` after every Edit, Write, or MultiEdit,
  on any file, not only `.rs`. Until a `Cargo.toml` exists it fails on every edit; the failure is noise,
  not a signal about the edited file.
- Zensical reads `zensical.toml` only from the current folder, so every docs command runs from the
  repository root.
