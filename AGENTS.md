# Kiri — Repository Contract

Layers on the global contract. Product truth: `docs/requirements/`. Architecture decision:
`docs/decisions/0001-architecture.md`. Backlog: https://github.com/users/TheoOdawara/projects/7, in
one-week sprints.

## Stack

- Rust, in a Cargo workspace (ADR 0001). No crate exists yet.
- Every dependency, the toolchain, the release targets, and what stays in the standard library are in
  `docs/decisions/0002-stack.md`. A dependency not listed there is added to it before the code uses it.

## Programming languages & scripts

- Application code: Rust.
- Repo scripts: Rust, as `cargo xtask`. No shell scripts.

## Architecture

Decided in ADR 0001. Nothing is built yet.

- Process model: `kiri daemon` owns every session, for every provider. There is one per user, started by
  the first client and exiting after an idle period with no running session and no client. The TUI,
  `kiri -p`, and `kiri acp` (the IDE stdio bridge) are ACP clients of it, and no client runs the engine.
  Transport: Unix domain socket on Linux/macOS, named pipe on Windows, restricted to the owning user.
- Crates, with one issue label each:

| Crate | Holds | Depends on | Public surface | Label |
|---|---|---|---|---|
| `kiri-core` | Engine and session host | no other Kiri crate | ACP types and `kiri/*` extension methods; the rest is `pub(crate)` | `area:core` |
| `kiri-tui` | Inline interface (full-screen later) | `kiri-core`'s protocol | — | `area:tui` |
| `kiri` | The binary: `daemon`, CLI, `-p`, `acp` | `kiri-core`, `kiri-tui` | — | `area:cli` |

- `kiri-core` is split by capability: `agent`, `provider`, `tools`, `permissions`, `sandbox`, `session`,
  `extensions`. Modules call each other directly; events exist only at the ACP boundary. Its two
  traits are `Provider`, one file per provider in `provider/`, and `Sandbox`, one file per OS in
  `sandbox/`, chosen at compile time.
- Streaming: SSE bytes → provider events → batches of about 10 ms → ACP events.
- Claude Pro/Max is an agent backend: the user's installed `claude` CLI in `-p` stream-json mode, its
  built-in tools disabled, Kiri's tools served to it through an MCP server, behind a minimum CLI version.
- A subagent is a child session in the daemon, on the same protocol.
- Extensions are data (Markdown skills, commands, agents) and child processes (hooks, MCP servers), in
  Claude Code's formats. Third-party code runs only outside the daemon, and nothing from a plugin or a
  project activates before the user trusts it.
- Persistence is text (JSONL, TOML, Markdown) under `~/.kiri` and `<project>/.kiri`.

## Conventions

Declared pending: stack-specific conventions are recorded here as the first crates land. The previous
codebase's conventions do not apply.

## Branches

- `main`: releases only. Never commit directly.
- `staging`: integration. Every work branch is cut from `staging` and merged into it by PR. The end-to-end
  test of a release runs on `staging` before it merges into `main`.
- Work happens on `feat/…`, `fix/…`, `chore/…` branches. Remote: `origin` (github.com/TheoOdawara/Kiri).
- Neither `main` nor `staging` has GitHub branch protection configured. The rules above are convention
  only.

## Language

English for code, identifiers, file names, comments, docs, `AGENTS.md`, commits, and branches. Interface
text is English until OQ-02 decides on localization.

## Gotchas

- `.claude/settings.json` runs `cargo fmt && cargo clippy --quiet` after every Edit, Write, or MultiEdit,
  on any file, not only `.rs`. Until a `Cargo.toml` exists it fails on every edit; the failure is noise,
  not a signal about the edited file.

## Project skills

- `gates`: every gate with its exact invocation, and how to serve the docs site. Use before calling work
  done.
