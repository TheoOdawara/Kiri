# ADR 0004 — Rename to Kiri and the TUI front-end

- Status: Accepted
- Date: 2026-06-21

## Context

The working name `T-Cli` was a placeholder. The harness is meant for real software engineers — code
that holds up to real engineering, with the model's reasoning, tool calls, and diffs fully visible and
under human control — not a vibe-coder hand-holding tool. The line-based REPL (the `repl` module:
`Terminal` + `Repl`) proves the engine but cannot deliver that experience: live transcript with
scrollback, inline diff review, a command palette, modes (plan/act), and inspectable context.

Two questions had to be settled together because the second depends on the first being a stable
identity: (a) a real name, and (b) how a full-screen TUI fits the modular-hexagonal architecture
(ADR 0003) without disturbing the engine.

## Decision

### Name: **Kiri** (桐)

The paulownia — the family kamon, and the wood of the chest (*kiri-dansu*) that protects a household's
most precious goods (the harness's safety-first thesis). Homophone with 切り ("cut", i.e. code). The
binary and command are `kiri`. The rename is strings/config only — package name, clap command name,
the system-prompt identity, and docs. The sandbox-root env var becomes `KIRI_PATH`. (The legacy
`T_CLI_PATH` fallback was kept transitionally and later removed pre-launch — no legacy users — so only
`KIRI_PATH` remains.)

### TUI: a new `tui` module, additive to the engine

The engine already exposes the UI as three push-style ports — `EventSink` (sync stream deltas),
`Presenter` (sync turn lifecycle), `ApprovalPolicy` (async approval) — all driven by `AgentLoop::run`.
The TUI is a **new bounded context** `src/modules/tui/{domain,application,infrastructure}`, peer to
`repl`. The existing `Repl`/`Terminal` are **kept unchanged** as the non-TTY / `--plain` / CI frontend;
`app::wire` selects between them by `stdout().is_terminal()` and a `--plain` flag. The TUI is therefore
purely additive and reversible at the composition root.

**The Elm Architecture.** The UI is a pure state machine: `domain` holds the `Model` (transcript
projection, input buffer, panels, overlay, status, mode, phase); `application` holds `Msg`, `Effect`,
`Command::parse`, the pure `update(&mut Model, Msg) -> Vec<Effect>`, and the pure key map. Only
`infrastructure` touches `ratatui`/`crossterm` (the `view`, widgets, the runtime). A CI grep forbids
`use ratatui`/`use crossterm` in `domain`/`application`, keeping the layer-inward invariant honest.

**The Bridge — the only new adapter.** A `Bridge` implements the three engine ports over channels:
the sync ports push `EngineMsg` onto an unbounded `mpsc`; `decide`/`confirm_continue` send a
`PendingApproval` carrying a `oneshot::Sender<Approval>` and await the reply. The engine handles tool
calls sequentially, so there is never more than one pending approval — the `oneshot` rides inside the
message, with no id/HashMap correlation. A dropped channel maps to `Io(BrokenPipe)` (sync) or
`Approval::Aborted` (async), reusing the engine's existing "end the session" semantics.

**Execution model (the load-bearing decision).** The agent-turn future is `!Send` (the ports are
`async_trait(?Send)`). It is **never spawned**: `main` keeps `#[tokio::main]`, and the whole app is one
`async fn run(self)` future. The turn is driven as a `tokio::select!` **arm** alongside the crossterm
`EventStream`, the engine `mpsc`, and a frame tick — so streaming renders live and input stays
responsive without any `Send`/`'static`/`LocalSet` machinery. **Cancellation is cooperative**: a
hand-rolled `CancelToken` (`Rc<Cell<bool>>`) is checked in `Bridge::on_event`; when set it returns
`Err`, which unwinds `provider.complete()` through the stream's existing `?`, letting the turn's error
path run conversation rollback. No engine change is required.

**Layout.** Hybrid, built so the single-pane core stands alone and panels are an additive, reversible
toggle layer (an empty `PanelSet` is exactly the core). Native palette (ANSI-16), hand-rolled input
editor, fuzzy palette, and diff/markdown rendering (native-over-deps); only `ratatui` and `crossterm`
are added.

**Scope boundary.** This milestone ships the complete TUI *front-end* shell. Facilitators that need
engine behavior — plan mode actually gating tools, `/test` running cargo, `/review` dispatching a
reviewer, `/spec`/`/adr` scaffolding, live `/model` switch, precise tokenization — are routed through a
single seam (`Effect::RunFacilitator`) that surfaces a marked notice and the relevant panel. The
backend wiring behind that seam is a later milestone.

## Consequences

- The engine (`agent`, `provider`, `tools`) is untouched; the TUI is one new module plus one branch in
  `app::wire`. The whole front-end is swappable, exactly as ADR 0003 anticipated.
- The pure `update`/`parse`/key-map/token-estimate functions are unit-testable; `ratatui::TestBackend`
  enables render snapshots; the existing `ScriptedProvider` drives Bridge/runtime integration tests.
  The frozen `characterization.json` and all current tests stay green through every phase.
- New dependencies: `ratatui` and `crossterm` (`event-stream`); everything else hand-rolled.
- Delivered in green-gated phases: rename → execution model + single-pane core → panels → modals & diff
  → facilitator surface. The runtime topology is proven first, before any chrome.
- Deferred behind the `Effect::RunFacilitator` seam: the engine-side behavior of plan mode, `/test`,
  `/review`, `/spec`, `/adr`, live `/model`, and precise token/cost accounting.
