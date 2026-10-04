# 0001. Architecture: daemon, ACP boundary, capability modules, data extensions
- Status: accepted
- Date: 2026-09-30

Supersedes the `Architecture` section of `CLAUDE.md` and every earlier ADR in this folder; all of them
describe the implementation discarded in the reset (see `docs/requirements/CHANGELOG.md`).

## Context

`docs/requirements/` sets the forces:

- One engine serves four front-ends: inline TUI (Milestone 1), full-screen TUI, headless `kiri -p`, and
  IDEs via ACP.
- Many provider kinds, including one — Claude Pro/Max — whose loop cannot be owned by Kiri (see Decision 7).
- OS confinement on Windows, Linux, and macOS, each with a different mechanism.
- Extensions: skills, commands, agents, hooks, MCP, plugins that bundle them.
- Future: sessions talking to each other and to their subagents.
- The previous codebase failed on ceremony (ports and layers for single implementations, six files per
  change), instability, poor UX, and platform workarounds.

Research on existing agents (opencode, openclaude, Zero, Grok Build, CLIProxyAPI, Meridian,
oh-my-openagent; 2026-09-30) informed the trade-offs; no choice below rests on "who uses it".

No single architecture style answers everything. The architecture is four answers to four questions:
the boundary between front-ends and engine, the physical structure, the engine's internals, and the
extension model.

## Decision

### 1. Process model — one daemon per user, owning every session

- `kiri daemon` hosts every session, for every provider. Front-ends are thin clients.
- The first client starts it; it stays alive while any session is running or any client is connected,
  and exits after an idle period. Closing a terminal does not end its session; a client can reattach.
- No embedded mode: every client — TUI, `kiri -p`, IDE bridge — goes through the daemon. One execution
  path.
- Transport: Unix domain socket on Linux/macOS, named pipe on Windows, both restricted to the owning user.

### 2. Boundary — ACP, event-driven

- Clients and daemon speak ACP (Agent Client Protocol, JSON-RPC). Clients send commands (prompt,
  approve, cancel, rewind); the engine emits events (text, tool call proposed, diff, approval needed,
  turn end). The engine never knows which front-end is attached.
- Kiri-specific operations (rewind, provider switch, session-to-session messaging) are ACP extension
  methods under `kiri/*`.
- IDE integration is `kiri acp`: a stdio bridge between the IDE and the daemon socket.
- Events stop at this boundary. Inside the engine, modules call each other directly.

### 3. Physical structure — a three-crate workspace

| Crate | Holds | Public surface |
|---|---|---|
| `kiri-core` | The engine and the daemon's session host | The protocol only |
| `kiri-tui` | Inline interface (full-screen later) | — |
| `kiri` | The binary: `daemon`, CLI, `-p`, `acp` bridge | — |

The compiler enforces the boundaries through visibility. A new crate appears only when it has a
consumer of its own.

### 4. Engine internals — modules by capability

- `kiri-core` is split into capability modules: `agent`, `provider`, `tools`, `permissions`, `sandbox`,
  `session`, `extensions`, `sync`. Each file groups one responsibility. No domain/application/
  infrastructure layering.
- A trait exists only with two or more real implementations: `Provider` (OpenAI-compatible, Anthropic,
  OpenAI Responses, ChatGPT subscription, Claude CLI backend) and `Sandbox` (one per OS, chosen at
  compile time).

### 5. Streaming — a local pipeline

Provider output flows as: raw SSE bytes → provider events → batches of about 10 ms → ACP events. Each
stage does one transformation and is testable alone; batching keeps redraws off the per-token path.

### 6. Extensions — data and child processes

- Plugins bundle skills, commands, and agents (Markdown), hooks (scripts), and MCP servers (processes),
  installed from git marketplaces. Formats are compatible with Claude Code's (`SKILL.md`, hook event
  names, `.claude/` discovery), so that ecosystem works from day one.
- Third-party code never runs inside the daemon — only as a child process. Code-level extension happens
  by shipping an MCP server.
- Nothing from a plugin or a project activates without the user's trust approval.

### 7. Providers — model providers and an agent backend

- **Model providers** (Kiri owns the loop): OpenAI-compatible, Anthropic and Anthropic-compatible,
  OpenAI Responses, local, and ChatGPT Plus/Pro via the Codex OAuth flow in-process.
- **Claude Pro/Max is an agent backend**: the engine runs the user's installed, logged-in `claude` CLI
  in `-p` stream-json mode, disables its built-in tools, and serves Kiri's tools to it through an MCP
  server. Claude's loop drives; Kiri's tools, permissions, and sandbox execute. The engine translates the
  CLI's stream into the same ACP events, so front-ends cannot tell the difference. Kiri checks a minimum
  CLI version.

### 8. Sessions and subagents

A subagent is a child session inside the daemon, speaking the same protocol. A client can attach to it,
watch it, and message it.

### 9. Persistence — text files under `~/.kiri`

Sessions, memory, configuration, and extensions are text (JSONL, TOML, Markdown) under `~/.kiri` and
the project's `.kiri/`. No SQLite: sync goes through git, and text diffs and merges where a database
file conflicts whole.

## Consequences

Easier:
- A new front-end is a protocol client; headless and IDE reuse everything.
- Sessions survive closed terminals; session-to-session and subagent conversations need no new
  infrastructure.
- A new provider is one file plus registration; a new OS sandbox is one implementation.
- Boundaries are compile errors, not review comments or guard tests.
- A broken plugin cannot crash or read the daemon.
- The Claude subscription path has a genuine client fingerprint instead of an impersonation to maintain.

Harder, and accepted:
- Daemon lifecycle (start, reattach, idle exit, stale socket) is Milestone 1 work, before any feature.
- Every client-engine interaction is serialized, even on one machine.
- ACP's shape constrains the protocol; anything outside it becomes a `kiri/*` extension method.
- With the Claude backend, Kiri does not own the loop: the built-in gate must work through the CLI's
  hooks, and behavior depends on the CLI's release cadence.
- No in-process code plugins: a plugin that needs code must ship an MCP server.

## Alternatives considered

- **Layered (UI → service → data)** — each of four front-ends would call services its own way; no
  extension story.
- **Hexagonal throughout** — a port per dependency even with one implementation is the ceremony that
  sank the previous codebase; kept only as "trait when two implementations exist".
- **Event bus between internal modules** — untraceable flow; events stay at the boundary.
- **Many crates (Codex ~150, Grok Build ~110)** — team-scale split; for one maintainer each crate is a
  public API to keep stable.
- **In-process engine, daemon later** — cheaper start, but session persistence and session-to-session
  messaging were wanted in the base.
- **Daemon plus embedded fallback (Grok Build, Zero)** — two execution paths to keep in sync.
- **Own protocol plus an ACP translator** — freedom in shape at the cost of a permanent translation
  layer for IDEs.
- **In-process code plugins (WASM host, or JS modules as in opencode)** — ABI, versioning, and isolating
  third-party code; opencode loads plugin code with full trust and no gate.
- **Claude Pro/Max via own impersonating proxy (CLIProxyAPI style)** — breaks on each Claude Code
  release (version floor, beta headers, body signature, TLS fingerprint); its Claude executor alone is
  about 29k lines of Go, changing weekly.
- **Claude Pro/Max via a CLIProxyAPI sidecar** — someone else's release pace, a second runtime, and a
  new local attack surface.
- **SQLite for sessions and memory** — a binary file conflicts whole under git sync.
