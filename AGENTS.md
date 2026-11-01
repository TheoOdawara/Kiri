# Kiri — Repository Contract

Layers on the global contract. Product truth: `docs/requirements/`. Architecture decision:
`docs/decisions/0001-architecture.md`. Backlog: <https://github.com/users/TheoOdawara/projects/7>, in
one-week sprints.

## Stack

- Rust, in a Cargo workspace (ADR 0001): `crates/kiri-core`, `crates/kiri-tui`, `crates/kiri`, `xtask`.
- Every dependency, the toolchain, the release targets, and what stays in the standard library are in
  `docs/decisions/0002-stack.md`. A dependency not listed there is added to it before the code uses it.

## Programming languages & scripts

- Application code: Rust.
- Repo scripts: Rust, as `cargo xtask`. No shell scripts.

## Architecture

Decided in ADR 0001. Built so far: the daemon on demand and `initialize` (#14), restricted to its OS user (#15), a text answer streamed from an OpenAI-compatible endpoint (#16), reasoning and tool calls in arrival order, in batches of about 10 ms (#17), the native catalog with request parameters (#18), and every failure reported with its cause (#19); the rest of this section is the target.

- Process model: `kiri daemon` owns every session, for every provider. There is one per user, started by
  the first client and exiting after an idle period with no running session and no client. The TUI,
  `kiri -p`, and `kiri acp` (the IDE stdio bridge) are ACP clients of it, and no client runs the engine.
  Transport: Unix domain socket on Linux/macOS, named pipe on Windows, restricted to the owning user.
- Crates, with one issue label each:

| Crate | Holds | Depends on | Public surface | Label |
| --- | --- | --- | --- | --- |
| `kiri-core` | Engine and session host | no other Kiri crate | ACP types, `_kiri/*` extension methods, `daemon::run`, `protocol::connect`; the rest is `pub(crate)` | `area:core` |
| `kiri-tui` | Inline interface (full-screen later) | `kiri-core`'s protocol | — | `area:tui` |
| `kiri` | The binary: `daemon`, CLI, `-p`, `acp` | `kiri-core`, `kiri-tui` | — | `area:cli` |

- `kiri-core` is split by capability: `protocol`, `agent`, `provider`, `tools`, `permissions`, `sandbox`, `session`,
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

- Versions live in `[workspace.dependencies]`, pinned with `=`; `Cargo.lock` is committed and the gates run
  with `--locked`.
- `kiri-core` errors are `thiserror`, and their `Display` is the user's message, word for word from the spec.
  `kiri` and `kiri-tui` use `anyhow` and print that message.
- Besides `shortcut:` markers, the only comments are `// SAFETY:`, required by `clippy::undocumented_unsafe_blocks`.
- `crates/kiri/tests/` runs the real binary against a temporary `HOME` and `USERPROFILE`.

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
  on any file, not only `.rs`.
- The Windows pipe name depends on the user's SID, not on the home. Tests that start a daemon therefore run one
  at a time, and each waits for the daemon's lock to free before ending.
- The daemon holds its lock until after it removes the socket, so an exiting daemon never deletes the socket of
  the next one.
- A Unix socket path of exactly the system's limit does not fit: `sun_path` also holds the terminating NUL.
- `kiri` ends its runtime with `shutdown_background`, because tokio's stdin holds a blocking thread that a normal
  drop would wait for. It spawns the daemon through `tokio::process`, whose reaper keeps a daemon that lost the
  lock from staying a zombie.

- The pipe's security descriptor names its owner. Left to the default, an elevated process creates the pipe owned by
  Administrators, and the client refuses its own daemon.
- `cargo xtask cross-user` keeps its home under `/tmp` and not `$TMPDIR`: on macOS `$TMPDIR` is private to its user,
  so `nobody` could not cross it even with the modes widened.
- A daemon with an invalid environment drops its listener and waits for `serve`'s confirmation before it sends the
  error. Sent first, the error lets a fixed client reach the old daemon before it stops listening.
- A turn updates the conversation and clears `running_turn` before its answer goes out. Otherwise a client's
  immediate next prompt gets "A turn is already running in this session."
- A batch is cleared only after its `send` returns, and a tool call is recorded only after its own. `run_turn`'s
  `select!` can drop the stream mid-send, and `finish` then sends what is still held, without repeating anything.
- A refusal happens inside `Listener::accept`, so `serve` never sees it. Counted as a client, it would keep a daemon
  with no real client alive forever.
- `reqwest`'s read timeout restarts when the headers arrive, and SSE comments reset it without reaching `eventsource-stream`'s
  events. The daemon can stay silent past 70 seconds on a healthy turn, so `_kiri/alive` is what keeps the client from
  calling the connection lost.

## Project skills

- `gates`: every gate with its exact invocation, and how to serve the docs site. Use before calling work
  done.
