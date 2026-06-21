# ADR 0003 — Modular hexagonal architecture for the harness

- Status: Accepted
- Date: 2026-06-21

## Context

The project grew from a single-turn chat client into an interactive agent harness: a streaming REPL,
tool-calling with a filesystem sandbox, and per-call human approval. Under the original layering
(`main → services → models`), `main.rs` had become a god-entry-point — env loading, clap parsing, the
REPL loop, the agent tool-loop, terminal rendering, and approval I/O all in one file — and
`services/tools.rs` a 1189-line file mixing every tool's schema, execution, and confirmation across
three central `match` statements. The coarse three-layer split no longer matched the harness's
concerns, and adding a tool or a provider meant editing several unrelated places.

We want a structure that (a) makes the agent loop a first-class, testable unit, (b) isolates each
external system (the LLM provider, the filesystem, the terminal) behind a port so it is swappable and
mockable, and (c) scales to the roadmap: multiple providers and persisted sessions.

## Decision

Adopt **modular hexagonal architecture** (ports & adapters, vertical slices), in the flavor of the
SEAP-RJ/SIGA backend: `src/shared/{kernel,infra}` + `src/modules/<context>/{domain,application,
infrastructure}` + a thin composition root.

**Layers (dependencies point inward).**

- `domain/` — pure data and rules, no I/O (e.g. `agent/domain`: `Role`, `Message`, `Conversation`,
  `StreamEvent`, `CompletedTurn`).
- `application/` — use-cases and the **ports** they depend on, expressed as **traits** (no `I` prefix,
  named by capability). The agent loop (`RunTurn`) lives here, depending on `CompletionProvider`, the
  `Tool`/`ToolRegistry` contract, and the UI port `AgentIo` (`EventSink` + `Presenter` +
  `ApprovalPolicy`).
- `infrastructure/` — **adapters** implementing the ports: the OpenAI-compatible provider (HTTP + SSE),
  the filesystem tools + sandbox, and the terminal.

**Modules (bounded contexts).** `agent` (conversation domain + the `RunTurn` use-case + UI ports),
`provider` (the `CompletionProvider` port + per-provider adapters + the OpenAI wire DTOs), `tools` (the
`Tool` trait + `ToolRegistry` + the sandbox + one fs adapter per tool), `repl` (the terminal + the REPL
driving adapter). `session` (SQLite-persisted conversations) is a planned vertical, not yet built.

**shared/kernel** holds cross-cutting primitives that more than two modules need: the protocol types
`ToolCall`/`FunctionCall`, and the typed `AgentError` (thiserror) that ports return. **shared/infra**
holds cross-cutting infrastructure: `config` (the CLI, env loading, `Settings`). The composition root
(`app::wire`) builds the concrete adapters and injects them into the use-cases; `main` is ~8 lines.

**Key boundary decisions.**

- The domain `Message` carries no serde; the provider owns a `MessageDto` (`From<&Message>`) so each
  provider can serialize messages its own way (the multi-provider seam).
- `ChatRequest.tools` is `Vec<serde_json::Value>` — the opaque schemas the registry produces — so the
  wire layer does not depend on a typed tool struct, and a tool's `schema()` is its own concern.
- The provider port streams via a callback (`EventSink`), not a `Stream`, to stay `dyn`-compatible for
  runtime provider swapping (`Arc<dyn CompletionProvider>`); `async-trait` boxes the async port methods.
- The terminal is a single owner of stdin/stdout, exposed to the engine through one unified `AgentIo`
  port (a supertrait of the three UI ports), so the agent loop borrows it once and never touches the
  console directly. `RunTurn::run<IO: AgentIo>` is generic over it (monomorphized, no trait upcasting).

**Invariants (enforced).** Network I/O only in `provider/infrastructure`; filesystem I/O only in
`tools/infrastructure` (the sandbox is the single path chokepoint); `domain` has no I/O; the engine
emits no direct `stdin`/`stdout`/`eprintln!`. Errors are typed `AgentError` across ports; `anyhow` is
used only at the binary edge.

This **supersedes the `main → services → models` rule** of ADR 0001/0002; the `models/` and `services/`
modules are removed.

## Consequences

- Adding a tool is one new file under `tools/infrastructure/fs/` implementing `Tool`, registered in
  `default_fs_tools` — no central match to edit. Adding a provider is one new adapter implementing
  `CompletionProvider`, chosen in `app::wire`.
- The agent loop is unit-testable without a network or a TTY: a `ScriptedProvider` + `ScriptedIo`
  integration test asserts the exact conversation sequence (user → assistant tool-calls → tool result →
  assistant text) and the abort path.
- The refactor was carried out behavior-preserving, in green-gated steps (`cargo fmt --check → clippy
  -D warnings → build → test` after each), guarded by a characterization snapshot test
  (`src/snapshots/characterization.json`) that froze every tool schema and confirmation string
  byte-for-byte.
- New dependencies: `async-trait` (dyn-compatible async ports), `thiserror` (`AgentError`).
- Deferred and additive under this structure (a new module / a new adapter, not a restructure): the
  `session` vertical (SQLite persistence) and the second provider adapter.
