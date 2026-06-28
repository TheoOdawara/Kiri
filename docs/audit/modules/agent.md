# Audit — Agent module

> Scope: `src/modules/agent.rs`, `src/modules/agent/application.rs`, and `src/modules/agent/application/{agent_loop.rs, approval_policy.rs, presenter.rs, tool_observer.rs}` (read in full)  
> Date: 2026-06-27  
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The agent module is in genuinely good shape: it is a clean application-only slice (ports as capability-named traits, no I/O, no `anyhow`, no `unwrap`/`expect`/`panic` on any runtime path, the provider error path is render-cleaned and tested, and every fallible `let _`/`unwrap_or` carries a justification or is a deliberate display fallback). The headline issues are all maintainability/clarity rather than correctness or security: the core `run` method is a 247-line, four-levels-deep function that would read better split; the test module repeats five near-identical UI doubles and the registry-construction snippet; and a family of tool-result strings are inline magic literals with mixed English/Portuguese within the same function. No Critical and no architecture-invariant violations were found — the auto-mode security guard (destructive + out-of-root calls still confirmed unattended) is sound and exhaustively tested.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 2 | 5 |

## Findings

### [AGENT-01] Split the 247-line `run` method; extract per-call decision and checkpoint handling
- **Severity:** High
- **Category:** file-size
- **Location:** `src/modules/agent/application/agent_loop.rs:111` through `src/modules/agent/application/agent_loop.rs:358`
- **Problem:** `AgentLoop::run` is a single ~247-line function carrying five distinct responsibilities (round streaming, plan-mode short-circuit, within-round runaway checkpoint, per-mode per-call decision, and result/abort bookkeeping) nested up to four levels deep (`loop` → `for call` → `match mode` → `match io.decide`). The contract demands "small & single-purpose" functions; this is the most-touched function in the module and its depth makes the control flow hard to follow and risky to change. The per-mode decision block alone (lines 229–306) is ~77 lines of nested matches that each independently call `io.tool_started` and `timed(...)`.
- **Evidence:**
```rust
for (index, call) in calls.iter().enumerate() {
    if calls_since_checkpoint >= self.max_tool_calls { /* ...checkpoint... */ }
    let command = self.registry.command_line(sandbox, call)
        .unwrap_or_else(|| call.function.name.clone());
    let result: Option<(ToolOutcome, Duration)> = match mode {
        ApprovalMode::Auto => match self.registry.confirm(sandbox, call) {
            Some(confirmation) if ... => match io.decide(&confirmation).await { /* 4 arms */ }
            _ => { io.tool_started(call, &command); Some(timed(...).await) }
        },
        ApprovalMode::Plan if !self.registry.is_plannable(...) => { /* ... */ }
        ApprovalMode::Plan => { /* ... */ }
        ApprovalMode::Default => match self.registry.confirm(sandbox, call) { /* 4 arms */ },
    };
```
- **Recommendation:** Extract two private async methods — e.g. `decide_and_run(&self, sandbox, call, &command, &mut mode, io) -> Option<(ToolOutcome, Duration)>` for lines 229–306, and a `checkpoint` helper that interprets an `Approval` into the `(reset | switch-to-auto | end)` transition shared by AGENT-04. `run` then reads as: stream round → plan check → for each call {checkpoint, decide_and_run, record} → post-round checkpoint. Do not implement here (scan-only).

### [AGENT-02] Collapse five near-identical test UI doubles into one configurable double
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/agent/application/agent_loop.rs:407` (`ScriptedIo`), `:439` (`CountingIo`), `:475` (`RecordingIo`), `:1224` (`FinishCountingIo`), `:1286` (`ReasonRecordingIo`)
- **Problem:** Five test doubles each implement all four UI ports, and their `EventSink::on_event`, `Presenter::begin_turn`/`finish_turn`, and `ToolObserver` impls are byte-for-byte the same boilerplate (`Ok(())` / `{}`). That is ~15 redundant trait-impl blocks. The differences between the doubles are tiny (how `decide`/`confirm_continue` answer, and which calls they count/record), so the boilerplate buries the one axis each test actually varies, and any port signature change forces five edits.
- **Evidence:**
```rust
impl EventSink for ScriptedIo {           // identical body appears 5x
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> { Ok(()) }
}
impl Presenter for ScriptedIo {           // identical body appears 5x
    fn begin_turn(&mut self) {}
    fn finish_turn(&mut self) -> Result<(), AgentError> { Ok(()) }
}
```
- **Recommendation:** Introduce a single `TestIo` test double with configurable behavior (a decision queue + optional recording `Vec`s + counters), or a small macro that emits the inert `EventSink`/`Presenter`/`ToolObserver` impls. Keep only the behavior that each test needs to assert on. Scan-only — no change applied.

### [AGENT-03] Extract inline tool-result strings to named constants; resolve the English/Portuguese mix
- **Severity:** Medium
- **Category:** magic
- **Location:** `src/modules/agent/application/agent_loop.rs:168`, `:169`, `:203`, `:211`, `:266`, `:314`
- **Problem:** The messages written back as `tool_result` content (and persisted into the conversation) are inline literals with an undeclared shared convention (the `"ignorada: …"` prefix repeated four times) and no named constant, so a reader cannot see the set at a glance and a typo in one would silently diverge. Worse, within the same `run` function the plan-block message is English (`"'{}' is blocked in plan mode …"`, line 266) while the abort/checkpoint messages are Portuguese (`"ignorada: execução interrompida no checkpoint"`, `"ignorada: sessão encerrada"`, `"ignorada: interrompida pelo usuário"`). The project's user/model-facing language is pt-BR (the same NDJSON convention appears at `runtime.rs:361`), so the English plan-block string is the outlier — an inconsistency in the text fed to the model and shown via `ToolObserver`.
- **Evidence:**
```rust
answer_unanswered(conversation, &calls[index..], "ignorada: execução interrompida no checkpoint");
// ...
answer_unanswered(conversation, &calls[index..], "ignorada: sessão encerrada");
// ...
ToolOutcome::Error(format!("'{}' is blocked in plan mode (not available for planning)", call.function.name)),
```
- **Recommendation:** Hoist the fixed messages to `const` items (e.g. `IGNORED_CHECKPOINT`, `IGNORED_SESSION_ENDED`, `IGNORED_USER_ABORT`, `PLAN_BLOCKED`) at module top, and align the plan-block text's language with the rest (pt-BR, to match the project convention) so all model-facing tool-result strings read consistently. Scan-only.

### [AGENT-04] De-duplicate the checkpoint `Approved`/`ApprovedAuto` reset logic
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/agent/application/agent_loop.rs:190`–`:198` and `src/modules/agent/application/agent_loop.rs:343`–`:351`
- **Problem:** The within-round and post-round runaway checkpoints handle the `confirm_continue` result with the same `Approved => reset` / `ApprovedAuto => switch to Auto + reset` arms duplicated verbatim. Only the `Declined`/`Aborted` arms differ (the within-round case must answer the remaining calls first). The duplicated reset/switch logic is easy to update in one place and forget the other.
- **Evidence:**
```rust
Approval::Approved => { checkpoint = Instant::now(); calls_since_checkpoint = 0; }
Approval::ApprovedAuto => { mode = ApprovalMode::Auto; checkpoint = Instant::now(); calls_since_checkpoint = 0; }
```
- **Recommendation:** Extract a helper (paired with AGENT-01) that, given the `Approval`, applies the shared `Approved`/`ApprovedAuto` state transition and returns whether to continue, leaving each call site to handle only its distinct end-of-turn arms. Scan-only.

### [AGENT-05] Reuse `registry_for_tests` instead of repeating the `default_fs_tools` construction
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/agent/application/agent_loop.rs:544`–`:548`, `:1079`–`:1083`, `:1111`–`:1115`, `:1387`–`:1391`
- **Problem:** The exact registry-construction snippet `ToolRegistry::new(default_fs_tools(Arc::from(Vec::<Regex>::new()), Arc::from(Vec::<Regex>::new()), false))` is written four times, including in `agent_loop_with` and two inline `AgentLoop::new` test sites, even though a `registry_for_tests()` helper already exists for exactly this. The intent (empty allow/deny regex lists, no extra flag) is repeated rather than named.
- **Evidence:**
```rust
fn registry_for_tests() -> ToolRegistry {
    ToolRegistry::new(default_fs_tools(
        Arc::from(Vec::<Regex>::new()),
        Arc::from(Vec::<Regex>::new()),
        false,
    ))
}
```
- **Recommendation:** Route `agent_loop_with` and the inline test constructors through `registry_for_tests()` so the registry shape is defined once. Scan-only.

### [AGENT-06] `Presenter::begin_turn`/`finish_turn` are named per-turn but fire per provider round
- **Severity:** Low
- **Category:** naming
- **Location:** `src/modules/agent/application/presenter.rs:9`–`:12` (with the call sites at `agent_loop.rs:125` and `:141`)
- **Problem:** The port's own doc comment admits the method names mislead: `begin_turn` fires "once per provider round, i.e. N times within one multi-round user turn … not once per user turn." A name that means the opposite of its lifecycle is exactly the kind of "magic" the contract warns against; the doc comment is load-bearing because the name alone would mislead an adapter author about how often state resets.
- **Evidence:**
```rust
/// Reset per-stream rendering state before a provider completion starts streaming. NOTE: this fires
/// once per provider round, i.e. N times within one multi-round user turn (before each
/// `provider.complete`), not once per user turn — name an adapter's state accordingly.
fn begin_turn(&mut self);
```
- **Recommendation:** Rename to `begin_round`/`finish_round` (matching the loop's actual per-`complete` cadence), which would let the clarifying NOTE be deleted. Coordinate the rename with the `Bridge` adapter. Scan-only.

### [AGENT-07] `ToolObserver` port is absent from the architecture doc's agent-module description
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/agent/application/tool_observer.rs:11` (port) vs `CLAUDE.md` agent-module entry
- **Problem:** `CLAUDE.md` describes the agent module as "the `AgentLoop` + the UI ports `Presenter`/`ApprovalPolicy`, plus the provider's `EventSink`," but `ToolObserver` is a first-class agent port required by `run`'s `IO` bound and implemented by `Bridge` (`tui/infrastructure/bridge.rs:123`). The architecture doc therefore under-describes the module's port surface, so a reader mapping the contract to the code finds an undocumented fourth port.
- **Evidence:**
```rust
pub trait ToolObserver {
    fn tool_started(&mut self, call: &ToolCall, command: &str);
    fn tool_finished(&mut self, call: &ToolCall, outcome: &ToolOutcome, elapsed: Duration);
}
```
- **Recommendation:** Add `ToolObserver` to the agent-module port list in `CLAUDE.md` (docs change, outside this scan's edit boundary — flag only).

### [AGENT-08] `schemas` is fixed at the initial `mode` and not recomputed when `mode` tightens to `Auto` mid-turn
- **Severity:** Low
- **Category:** architecture
- **Location:** `src/modules/agent/application/agent_loop.rs:118`–`:121` (with the `mode` mutations at `:195`, `:291`, `:349`)
- **Problem:** `schemas` is computed once from the entry `mode`; when a Plan-mode turn later flips to `Auto` (via an `ApprovedAuto` at a checkpoint), subsequent provider rounds keep the plan-restricted schema, so destructive tools stay unadvertised. That outcome is safe (more restrictive, not less), but the comment only says the set is "fixed for the turn" — the security-relevant consequence (a Plan→Auto turn never gains destructive tools) is implicit and relied upon without being stated, so a future "recompute schemas on mode change" refactor could silently widen the plan-mode tool surface.
- **Evidence:**
```rust
// The advertised tool set is fixed for the turn; in plan mode it excludes destructive tools.
// `mode` may still tighten to `Auto` mid-turn if the user approves a call with "don't ask again".
let mut mode = mode;
let schemas = self.registry.schemas_for(mode);
```
- **Recommendation:** Extend the comment to state the safety rationale (Plan→Auto deliberately keeps the plan-restricted schema; never recompute schemas on a mid-turn mode change), so the invariant is explicit and protected against refactors. Scan-only.

## Strengths
- **Disciplined error handling:** no `unwrap`/`expect`/`panic` on any runtime-reachable path; the one `let _ = io.finish_turn()` (line 141) carries a precise justification, and the provider failure path is render-cleaned *and* covered by a dedicated regression test (`provider_failure_propagates_after_finishing_the_render`).
- **Clean hexagonal boundaries:** the module is pure application — ports are capability-named traits (no `I` prefix), there is zero I/O, zero `anyhow`, and the engine never touches stdin/stdout (all UI flows through `EventSink`/`Presenter`/`ApprovalPolicy`/`ToolObserver`).
- **Security invariant is real and tested:** the auto-mode guard that still confirms destructive and out-of-root calls — and the within-round runaway cap against a prompt-injected burst — are each locked by explicit tests (`auto_mode_confirms_destructive_delete`, `auto_mode_confirms_out_of_root_target`, `the_call_cap_pauses_within_a_single_oversized_round`), and every early-exit path answers all pending tool calls to keep the exchange persistable.
