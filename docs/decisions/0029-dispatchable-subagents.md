# ADR 0029 — Dispatchable subagents: a read-only `task` tool over a nested `AgentLoop`

- Status: Accepted
- Date: 2026-07-06
- Amends: ADR 0021 (`0021-extensions-framework-and-trust-gating.md`) — activates `AgentProfile`'s
  `model`/`allowed_tools` fields, previously dead code, and overturns its doc comment stating an
  `AgentProfile` is "not an isolated sub-agent."

## Context

Before this ADR, an `AgentProfile` was only ever prepended as a system prompt to a command it was bound
to (`ExtensionCatalog::command_bodies`) — the same `AgentLoop`, the same turn, the full parent toolset.
There was no way for the model to hand off a self-contained sub-task (find something, design a plan) to
an isolated context the way Claude Code's own Task/Agent tool does. `AgentProfile::model` and
`::allowed_tools` existed in the domain type and the frontmatter parser but were never read by anything.

## Decision

Add a `task` tool (`agent::infrastructure::task_tool::TaskTool`) that, given a loaded agent's id and a
prompt, runs a **nested `AgentLoop`** — the existing loop, reused verbatim, not a variant — seeded with
the profile's `system_prompt`, scoped to a filtered tool subset, and returning only its final assistant
text to the parent as the tool result.

**Why `agent::infrastructure`, not `tools::infrastructure`.** The tool has to construct and drive an
`AgentLoop`, which needs a `CompletionProvider` and the engine's IO ports — reaching those from a plain
`tools/infrastructure` fs tool would invert the `tools -> agent` dependency direction. `agent` gained an
`infrastructure` layer for exactly this one adapter; its dependency on `tools::application`/
`provider::application` is inward/sideways, the same direction `AgentLoop` itself already depends in.

**v1 is deliberately narrow — read-only only, sequential, headless, structurally depth-1:**

- **Read-only intersection is the security boundary.** A subagent's toolset is `child_tools ∩
  profile.allowed_tools ∩ is_read_only()` (empty `allowed_tools` means "every read-only tool"). Even a
  profile that names `write_file`/`run_command` in `allowed-tools` never gets them in v1 — a headless
  subagent has no live user to confirm an irreversible action, so write/exec subagents are out of scope
  until approval-forwarding exists (see Deferred, below).
- **`HeadlessIo`** implements the engine's four IO ports for a subagent with nobody watching:
  `EventSink`/`Presenter`/`ToolObserver` are no-ops (the subagent's stream never reaches the parent
  transcript in v1, so it cannot interleave with and corrupt the parent's); `ApprovalPolicy::decide`
  approves only what the ordinary auto-mode gate would default-accept (an in-root read) and declines
  everything else outright — preserving SEC-01: an out-of-root target like `~/.ssh/id_rsa` is refused, not
  silently confirmed. `confirm_continue` always declines, ending a runaway subagent at its first
  checkpoint rather than running unbounded.
- **Structural depth-1.** The child tool pool a `TaskTool` draws from is built and cloned *before* the
  `task` tool itself is pushed into the registry (`app::wire`), so it never contains `task` — a subagent
  physically cannot dispatch another one. `tools_for` additionally filters `tool.name() != "task"` as a
  cheap belt-and-suspenders check.
- **Sequential only.** Multiple `task` calls in one assistant message already run one after another in
  `AgentLoop`'s existing per-call loop; there is no parallel fan-out. The engine's IO ports are `(?Send)`
  (single-threaded), so concurrent subagents would need a `Send`-safe engine — out of scope here.
- **Same-provider model only.** `profile.model` (falling back to the session's active model) is passed as
  the nested loop's model string against the *same* `CompletionProvider` adapter — a different
  provider/endpoint per subagent is not supported; Kiri's provider-agnostic-by-API-key design (ADR 0011)
  has no per-call provider-registry concept yet.
- Built only when `!extensions.agents.is_empty()` (`app::wire`), so an install with no agent profile
  loaded never advertises a dead tool.

**Tool sharing: `ToolRegistry` moves from `Vec<Box<dyn Tool>>` to `Vec<Arc<dyn Tool>>`.** A subagent's
filtered registry needs to share tool *instances* with the parent's — rebuilding a second copy would
double-connect anything stateful (an MCP proxy's live connection, a memory tool's store handle). This
ripples mechanically through every `default_*_tools` builder and `app::build_mcp_tools`; the compiler
pins every site.

## Consequences

- `AgentProfile::model`/`::allowed_tools` are live fields; their `#[allow(dead_code)]` is removed and the
  struct's doc comment now describes both consumption paths (command-binding prompt overlay, and `task`
  dispatch).
- `Tool` is now held behind `Arc`, not `Box`, across the whole tool layer — a wider but purely mechanical
  diff; no `Tool` trait method or tool behavior changed.
- Locked by `task_tool.rs`'s test contract: the read-only intersection actually drops a disallowed write
  tool (the security-relevant test), the child pool structurally excludes `task`, an empty `allowed_tools`
  yields every read-only tool, a scripted provider's answer returns to the parent as `Ok`, a provider
  error surfaces as `ToolOutcome::Error` (never a panic or a propagated `Err`), and `HeadlessIo` approves
  only `default_accept` confirmations and declines the runaway checkpoint.

## Deferred (explicitly out of scope for this ADR)

- **Write/exec subagents.** Requires forwarding a subagent's tool confirmation to the *real* user — a new
  engine-level port so `TaskTool::execute` can reach the runtime's approval channel (today `EngineMsg` is
  `pub(crate)` inside `tui::infrastructure`, unreachable from `agent`). Until that port exists, a
  dispatched subagent stays read-only regardless of what its profile's `allowed-tools` lists.
- **Streamed subagent output in the parent transcript.** v1 shows only the parent's `task` call
  start/finish and the subagent's final text as the tool result — no intermediate narration. Forwarding
  needs the same new port as write/exec approval.
- **Parallel fan-out** (multiple subagents running concurrently) — needs a `Send`-safe engine.
- **Multi-level dispatch** (a subagent dispatching a subagent) — needs a real depth counter threaded
  through, not just the structural depth-1 cap.
- **Cross-provider subagents** (a different provider/endpoint, not just a different model on the same
  provider) — needs a provider registry the composition root can hand a `TaskTool` beyond its single
  boot-time `Arc<dyn CompletionProvider>`.
