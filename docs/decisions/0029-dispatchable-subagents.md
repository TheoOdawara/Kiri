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

## Amendment (2026-07-07) — agent discoverability: the `# Agents` section this ADR always assumed

`TaskTool::schema()` has, since this ADR's original text, told the model to pick an agent id "as listed in
'# Agents'" (the schema quoted above). That section never existed: `render_system_prompt` took no `agents`
parameter, `ExtensionCatalog` had no `agents_index`, and `AgentProfile` had no `description` field to index
in the first place. A model with `search`/`planning` loaded could dispatch `task`, but only by guessing an
id and reading `TaskTool::execute`'s "unknown agent 'x'; loaded agents: planning, search" error — the
feature was invocable but not discoverable, the same gap `use_skill`/`# Skills` never had.

Closed by mirroring the skill-discovery chain exactly:

- `AgentProfile` gained `description` (frontmatter `description:`), read the same way `Skill.description`
  already was.
- `ExtensionCatalog::agents_index()` mirrors `skills_index()`: one `- {id} — {description}` line per agent,
  sorted by id, keyed by `id` (never the new display-only `name` — see ADR 0028's amendment — so the string
  the model reads in `# Agents` is always exactly what `task`'s `agent` parameter expects).
- `render_system_prompt` grew a `PromptExtensions` struct (`rules`/`skills`/`agents`/`instructions`) instead
  of a fifth positional parameter, since a fifth `Option<&str>` tripped clippy's `too_many_arguments` —
  grouping them mirrors the `ExtensionCatalog`-as-accumulator pattern `file_loader::load_type` already uses
  for the same lint. The rendered order is unchanged in spirit: Rules → Skills → **Agents** → Instructions →
  Security, so Security still always has the last, precedence-holding word.
- The two bundled agents (`search`, `planning`) both gained a `description` — an empty one would have shipped
  a broken index line, the same failure mode `bundled_skills_have_a_nonempty_description` already guarded
  against for skills; a matching `bundled_agents_have_a_nonempty_description` now guards agents.

No change to `TaskTool` itself (the schema's `# Agents` reference was already correct; it just pointed at
nothing) and no change to the v1 boundaries above (read-only, sequential, headless, depth-1) — this
amendment is pure discoverability, not a capability expansion.

## Trust posture (2026-07-07) — project agents are passive-trusted; containment is the read-only boundary

Making agents discoverable means a **project-layer** profile (`<workspace>/.kiri/agents/`) is now both
model-dispatchable and injected into the system prompt (its `description` into `# Agents`, its
`system_prompt` becoming a subagent persona). Unlike the two *active* capability types — hooks and MCP
servers, which pass through the ADR 0021 TOFU trust gate (`gate::resolve`) before they may run — agents,
like rules/skills/commands, are **passive resources: trusted on load, not gated**. A hostile
`<workspace>/.kiri/agents/evil.md` in a cloned repo is therefore loaded and dispatchable without an
approval prompt. This is deliberate and consistent with the existing ADR 0021 posture for the other
passive types; PR #64 extends the same untrusted-injection surface to agents rather than inventing a new
one. Containment does not rest on the gate — it rests on two independent invariants:

- **The subagent is read-only and in-root.** Whatever persona a malicious profile injects, the dispatched
  loop can only ever hold the read-only tool intersection, and `HeadlessIo` refuses every out-of-root
  target (SEC-01). The read-only surface is now locked by a guard test per tool-set builder
  (`default_fs_tools`/`default_memory_tools`/`default_extension_tools`), so a future read-only tool that
  does not self-gate an agent-supplied path cannot silently widen what a subagent can read.
- **`# Security` renders last.** A profile's `system_prompt` and `description` land in earlier prompt
  sections; the harness Security block always follows and holds precedence (same single-pass guarantee ADR
  0007/0019 give user instructions).

Two operational notes that follow from v1's headless design, worth stating so they are not mistaken for
defects:

- **A subagent always runs `ApprovalMode::Auto`**, even when the parent session is in Default or Plan mode
  (`task_tool.rs` hardcodes it). Approving a single `task` dispatch therefore grants the subagent
  unattended in-root reads for its whole lifetime. No privilege is gained (read-only, in-root; out-of-root
  is declined), but the trust nuance is "one approval → many unattended reads," not "one approval → one
  read."
- **`task` is plannable** (`is_read_only() == true`), so a subagent can be dispatched during plan mode and
  will make real, billable provider calls while the parent is nominally only planning. Bounded by the
  parent's `max_tool_calls` and the checkpoint budget, and desirable for the `planning` agent, but real
  spend nonetheless.

Folding passive project resources under the TOFU gate as well is a possible future tightening, deferred
here: it would add an approval prompt to first use of any project rule/skill/command/agent, a UX cost the
read-only + Security-last containment does not currently justify.
