# ADR 0005 — Approval modes, the plan flow, and retiring the plain REPL

- Status: Accepted
- Date: 2026-06-23

## Context

ADR 0004 shipped the TUI as a *front-end shell*: the slash grammar only covered `/exit`, approval was a
single line at the bottom of the screen, and "plan mode" was a label with no engine behavior — explicitly
deferred behind a facilitator seam. Two front-ends coexisted: the TUI on a TTY and the line-based `repl`
(`Repl` + `Terminal`) for non-TTY/`--plain`. This milestone makes the commands and modes real and
consolidates on one front-end.

## Decision

### Retire the plain REPL

The `repl` module and the `--plain` flag are removed; the TUI is the sole front-end, assembled in
`app::wire`, which now fails fast with a clear error when stdout is not a terminal. The REPL's only
unique behaviors are preserved where they belong: `/cd` becomes a TUI command backed by
`Sandbox::relocated`, and the dead engine-notice port (`Presenter::notice` → `EngineMsg::Notice`) it alone
used is dropped. Non-interactive (pipe/CI) execution is no longer supported; a headless TUI mode is a
possible future addition.

### Three approval modes, cycled with `Shift+Tab`

`ApprovalMode { Default, Auto, Plan }` (in `agent::application::approval_policy`) is the session's
execution policy. It lives on the TUI `Model` (the user owns it; it is shown on the meta rule) and is read
at the **start of each turn**, then passed into `AgentLoop::run(mode, …)`. Cycling mid-turn applies to the
next turn — a deliberate simplicity trade-off over sharing mutable mode with the engine.

Gating is centralized in the agent loop:

- **Default** — confirm every call through the UI (the prior behavior).
- **Auto** — execute every call without asking.
- **Plan** — advertise only read-only tools, so the model cannot even request a destructive action; a
  destructive call that slips through is refused without touching the filesystem.

Tools self-classify via `Tool::is_read_only` (default `false`, so a new tool is gated until it opts in);
`ToolRegistry` gains `schemas_for(mode)` and `is_destructive(name)`. The cached schema vector is dropped in
favor of per-turn computation (cheap for nine tools).

### Real slash commands

`command::parse` returns a richer `Command` and treats any `/`-prefixed line as a command — matched, or
`Unknown` (warned, not sent to the model). Commands: `/new` (fresh session), `/help`, `/cd [path]`, and
`/plan`/`/auto`/`/default`. Effects that need the runtime (`NewSession`, `ChangeWorkspace`) are handled
there; `Help` and mode switches are pure `Model` mutations.

### Rich approval box and the plan flow

The single-line prompt becomes a modal overlay (the market-standard pattern): the action plus selectable
options navigated with `↑`/`↓`+`Enter` or digits. The "don't ask again" option switches to auto. `Esc`/`n`
now declines just the call (was: abort); `Ctrl+C` aborts the session.

Plan mode is a loop: a finished plan-mode turn sets `Model.pending_plan`, surfacing a plan box —
**execute** (leave plan mode, run a turn that carries it out via `Effect::ApprovePlan`), **keep planning**,
or **cancel**. The choice/box machinery is shared with the approval widget.

## Consequences

- The engine stays the seam it always was: `run` takes one extra `mode` argument; all gating reads from
  the registry. No provider change.
- One front-end to maintain; the non-TTY path is gone until a headless mode is added.
- Behavior change: `Esc` declines rather than aborts; an unknown `/command` is flagged rather than sent to
  the model. Both are documented in the README.
- Pure functions (`command::parse`, `keymap`, `update`, `schemas_for`) and `ratatui::TestBackend` renders
  keep every phase green; the frozen `characterization.json` is unaffected (tool schemas and confirmation
  strings are unchanged).
- Realizes the plan-mode engine behavior ADR 0004 deferred; the broader facilitator seam (`/test`,
  `/review`, `/spec`, live `/model`) remains future work.
