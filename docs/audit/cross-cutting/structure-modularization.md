# Audit — File Size & Folder Organization Plan

> Scope: the whole `src/` tree (133 `.rs` files, 28,016 lines) — line-count census via `wc -l`, structural skeletons via `grep`, full reads of the largest production files (`runtime.rs`, `keymap.rs`, `config.rs`, `view_state.rs`, `markdown.rs`, `sync_service.rs`, `app.rs`, `agent_loop.rs`).
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary

The hexagonal layering is clean and the module tree is logical, but the whole-graph view reveals one true god-file and a handful of grab-bag files that have outgrown a single responsibility. The headline is **`tui/infrastructure/runtime.rs` — ~1,627 production lines** holding the event loop, provider swapping, session ops, distillation, sync, and render glue, with a single **~410-line `Tui::run` method**. A critical methodological note for any reader of a raw `wc -l` census: **the crate is ~17.2k production / ~10.8k test lines**, and several "huge" files are almost entirely inline tests (`agent_loop.rs` is 360 prod / 1,071 test; `registry.rs` 94 / 542; `sandbox.rs` 350 / 313) — those do **not** need a production split, but their oversized inline `#[cfg(test)]` modules should be extracted so file size reflects real complexity. Two genuine inconsistencies span the graph: a **split module-file convention** (`mod.rs` in memory/sync/session vs `<name>.rs` everywhere else) and a **`view_state.rs` / `config.rs` grab-bag** of unrelated types. No security or correctness issues — this area is pure maintainability.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 2 | 7 | 5 |

## Census (files over ~300 lines, worst first)

| File | Total | Prod≈ | Test≈ | Verdict |
|---|---|---|---|---|
| `tui/application/keymap.rs` | 2274 | 946 | 1328 | split prod + extract tests |
| `tui/infrastructure/runtime.rs` | 2072 | **1627** | 445 | **god-file — split (STRUCT-01/02)** |
| `agent/application/agent_loop.rs` | 1431 | 360 | 1071 | prod OK; extract tests |
| `shared/infra/config.rs` | 1054 | 827 | 227 | split (STRUCT-03) |
| `tui/domain/view_state.rs` | 767 | 616 | 151 | grab-bag — split (STRUCT-05) |
| `sync/application/sync_service.rs` | 665 | 367 | 298 | extract Trust DTOs (STRUCT-12) |
| `tools/infrastructure/sandbox.rs` | 663 | 350 | 313 | prod OK; extract tests |
| `tui/infrastructure/markdown.rs` | 654 | 521 | 133 | split parse/render (STRUCT-10) |
| `tools/application/registry.rs` | 636 | 94 | 542 | prod tiny; extract tests |
| `tools/infrastructure/fs/run_command.rs` | 586 | 226 | 360 | prod OK; extract tests |
| `memory/application/memory_port.rs` | 554 | 222 | 332 | prod OK; extract tests |
| `session/infrastructure/sqlite_session_store.rs` | 506 | 370 | 136 | borderline OK |
| `app.rs` | 506 | 425 | ~80 | borderline OK |
| `memory/infrastructure/file_project_memory.rs` | 483 | 347 | 136 | OK |
| `provider/infrastructure/openai/sse.rs` | 482 | 199 | 283 | prod OK |
| `tui/infrastructure/widgets/transcript_pane.rs` | 474 | 410 | 64 | borderline |
| `memory/infrastructure/sqlite_shared_memory.rs` | 460 | 346 | 114 | OK |
| `memory/application/distill.rs` | 439 | 272 | 167 | OK |
| `provider/infrastructure/anthropic/message_dto.rs` | 389 | 199 | 190 | OK |
| `tui/infrastructure/widgets/editor.rs` | 372 | 218 | 154 | OK |
| `tools/infrastructure/fs/search.rs` | 371 | 245 | 126 | OK |
| `provider/infrastructure/anthropic/sse.rs` | 344 | 160 | 184 | OK |
| `shared/kernel/provider.rs` | 342 | 229 | 113 | OK |
| `tools/infrastructure/exec.rs` | 325 | 191 | 134 | OK |
| `provider/infrastructure/openai/provider.rs` | 319 | 119 | 200 | OK |
| `provider/infrastructure/factory.rs` | 313 | 176 | 137 | OK |
| `tui/infrastructure/view.rs` | 416 | 85 | 331 | prod tiny; extract tests |
| `tui/infrastructure/widgets/selection_overlay.rs` | 303 | 148 | 155 | OK |

## Findings

### [STRUCT-01] Split the `runtime.rs` god-file by responsibility
- **Severity:** High
- **Category:** file-size
- **Location:** `src/modules/tui/infrastructure/runtime.rs:79-1626` (entire production span)
- **Problem:** At ~1,627 production lines this is by far the largest unit of code in the crate and the only true god-file. It bundles at least six unrelated responsibilities behind one module: provider switching (`ProviderSwap` + `apply_set_model`/`apply_set_effort`/`apply_set_provider`/`apply_save_provider`), the terminal event loop (`Tui::run`), session lifecycle (`flush_session`/`list_sessions`/`open_session`/`rebuild_transcript`/`short_timestamp`), end-of-session learning (`should_distill`/`DistillStep`/`drive_distillation`), profile sync (`sync_push`), the turn driver (`drive_turn`/`on_turn_end`/`turn_produced_nothing`/`engine_msg`), and render/clipboard glue (`draw_and_copy`/`copy_to_clipboard`/`place_cursor`/`paste_from_clipboard`/`spinner_frame`). A reader cannot hold the file in their head, and any change touches an oversized blast radius.
- **Evidence:** the production skeleton (one module, all of these as siblings):
```rust
pub struct ProviderSwap { /* ... */ }           // :79   provider switching
pub struct Tui { /* ... */ }                     // :237  event loop owner
    pub async fn run(self) -> Result<()> { /* ~410 lines */ } // :301
async fn drive_turn(/* ... */)                   // :814  turn driver
fn apply_set_model(/* ... */)                    // :961  provider effects
async fn flush_session(/* ... */)                // :1148 session ops
async fn sync_push(/* ... */)                    // :1352 sync
async fn drive_distillation(/* ... */)           // :1443 distillation
fn on_turn_end(/* ... */)                         // :1548 turn lifecycle
```
- **Recommendation:** promote `runtime.rs` to a `runtime/` directory (keeping `runtime.rs` as the module facade per the crate's `<name>.rs` convention) and split into: `runtime/provider_swap.rs` (`ProviderSwap` + the four `apply_*` effect handlers), `runtime/session_ops.rs` (`flush_session`/`list_sessions`/`open_session`/`rebuild_transcript`/`short_timestamp`), `runtime/distill.rs` (`should_distill`/`DistillStep`/`drive_distillation`), `runtime/turn.rs` (`drive_turn`/`on_turn_end`/`turn_produced_nothing`/`engine_msg`/`Step`), `runtime/render.rs` (`draw_and_copy`/`copy_to_clipboard`/`place_cursor`/`paste_from_clipboard`/`spinner_frame`/`forces_draw`), and keep `Tui` + `run` (see STRUCT-02) in `runtime.rs`. Scan-only; do not implement.

### [STRUCT-02] Extract the ~410-line `Tui::run` event loop into per-effect handlers
- **Severity:** High
- **Category:** file-size
- **Location:** `src/modules/tui/infrastructure/runtime.rs:301-710`
- **Problem:** A single async method spans ~410 lines and contains the entire input/effect loop with an 18-arm inline `match effect { … }`. Several arms inline 30–60 lines of business logic (`SubmitPrompt`, `ResumeLast`, `OpenSession`, `ChangeWorkspace`, `NewSession`, `ApprovePlan`) while four others already delegate to `apply_*` helpers — an inconsistent depth that obscures the loop's actual control flow and makes the method untestable as a unit. This is the largest single function in the crate.
- **Evidence:** the inline effect dispatch (line offsets within `run`):
```rust
match effect {
    Effect::SubmitPrompt { text, images } => { /* ~30 lines inline */ }   // :125
    Effect::CopyToClipboard(text) => copy_to_clipboard(&mut model, &text),// :156 (delegates)
    Effect::ResumeLast => { /* ~37 lines inline */ }                      // :193
    Effect::OpenSession(id) => { /* ~25 lines inline */ }                 // :240
    Effect::ChangeWorkspace(path) => match sandbox.relocated(&path) { … } // :265
    Effect::SetModel(model_id) => apply_set_model(/* ... */),             // :339 (delegates)
    // … 18 arms total, mixed inline vs delegated
}
```
- **Recommendation:** factor the body of the `match effect` into a dedicated `async fn apply_effect(effect, &mut model, …) -> ControlFlow` (or one small handler per inline arm, mirroring the existing `apply_set_*` style) so `run` becomes a thin select/dispatch/draw loop. This pairs with STRUCT-01's `runtime/` split. Scan-only.

### [STRUCT-03] Split `config.rs` — Raw DTOs, resolution, writers, CLI, and Settings are five concerns in one file
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/shared/infra/config.rs:219-808`
- **Problem:** ~827 production lines hold five distinct responsibilities: the `Raw*` TOML deserialization DTOs (`RawConfig`/`RawHttp`/`RawBehavior`/`RawSandbox`/`RawPaths`/`RawEmbeddings` + their `is_empty` impls), env/CLI resolution helpers (`parse_duration_ms`/`resolve_timeout`/`parse_bool`/`resolve_bool`/`resolve_sandbox_mode`/`resolve_sandbox_network`/`expand_home`/`compile_patterns`/`load_*`), the global-config writers (`persist_active_model`/`persist_effort`/`persist_active_provider`/`upsert_provider`/`update_global_config`/`write_starter_config`/`default_provider`), the clap CLI surface (`Cli`/`CliCommand`/`SyncAction`), and the public `Settings`/`EmbeddingSettings` + `resolve`. These change for entirely different reasons.
- **Evidence:**
```rust
struct RawConfig { /* ... */ }                              // :219  deserialization DTOs
fn parse_duration_ms(raw: Option<&str>, default: Duration)  // :438  resolution helpers
pub fn upsert_provider(config_path: &Path, …) -> Result<()> // :404  writers
pub struct Cli { /* clap derive */ }                        // :560  CLI surface
pub struct Settings { /* ... */ }                           // :605  public resolved config
```
- **Recommendation:** promote to a `config/` directory: `config/raw.rs` (the `Raw*` DTOs + `is_empty`), `config/resolve.rs` (the parse/resolve helpers), `config/writers.rs` (the `persist_*`/`upsert_provider`/`update_global_config`/`write_starter_config`/`default_provider`), `config/cli.rs` (`Cli`/`CliCommand`/`SyncAction`), and keep `Settings`/`EmbeddingSettings`/`resolve` in `config.rs` as the facade. Scan-only.

### [STRUCT-04] Split `keymap.rs` production code + extract its 1,328-line test module
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/modules/tui/application/keymap.rs:32-945` (prod) and `:947-2274` (tests)
- **Problem:** 2,274 lines total — the largest file in the crate by raw count. The ~946 production lines are themselves a candidate for a split (editor/key dispatch, slash-menu, submit/command dispatch, the three modal handlers, and the provider wizard are five cohesive groups), and the remaining ~1,328 lines are a single inline `#[cfg(test)]` module that triples the file's apparent size.
- **Evidence:** five cohesive function clusters in one file:
```rust
pub fn on_key(model, key) -> Vec<Effect>           // :32   editor/key dispatch + on_mouse/to_input
pub fn sync_menu(model)                            // :348  slash-command menu
fn submit(model) -> Vec<Effect>                    // :414  submit + open_*_picker
fn on_approval_key / on_plan_key / on_picker_key   // :573  modal handlers
fn on_wizard_key / advance_wizard                  // :777  provider wizard
#[cfg(test)] mod tests { /* 1328 lines */ }        // :947
```
- **Recommendation:** promote to `keymap/` with `keymap/editor_input.rs` (`on_key`/`on_mouse`/`click_granularity`/`to_input`), `keymap/menu.rs` (`sync_menu`/`on_menu_key`/`complete_command`), `keymap/submit.rs` (`submit` + the `open_*_picker` helpers), `keymap/modals.rs` (`on_approval_key`/`on_plan_key`/`on_picker_key` + `Choice`), `keymap/wizard.rs` (`on_wizard_key`/`advance_wizard`); move tests to a sibling `keymap/tests.rs` via `#[cfg(test)] mod tests;` (see STRUCT-07). Scan-only.

### [STRUCT-05] `view_state.rs` is a grab-bag of unrelated TUI domain types under a misleading name
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/modules/tui/domain/view_state.rs:11-616`
- **Problem:** ~616 production lines hold ~15 unrelated value types: `ImageAttachment`, `InputBuffer` (a 116-line text-editing abstraction), `Granularity`/`SelectionState`/`ScreenSelection`, `History`, `Scroll`, `PendingApproval`/`PendingPlan` (+ their `*_OPTIONS` consts), `PickerKind`/`Picker`, and the `ProviderWizard`/`WizardStep`/`WIZARD_KINDS` cluster (which has a custom `Debug` and a `Drop` that zeroizes the key). "view_state" describes none of these — it is a dumping ground. Unrelated types changing in one file inflate diffs and review surface.
- **Evidence:**
```rust
pub struct InputBuffer { /* 116-line editor */ }   // :22
pub struct ScreenSelection { /* mouse selection */ } // :175
pub struct History { /* command recall */ }        // :219
pub struct PendingApproval { /* modal */ }         // :303
pub struct ProviderWizard { /* + Drop zeroize */ } // :395
```
- **Recommendation:** split into named domain files: `domain/input_buffer.rs` (`InputBuffer` + `ImageAttachment`), `domain/selection.rs` (`Granularity`/`SelectionState`/`ScreenSelection`), `domain/history.rs` (`History` + `Scroll`), `domain/modal.rs` (`PendingApproval`/`PendingPlan` + `APPROVAL_OPTIONS`/`PLAN_OPTIONS`), `domain/picker.rs` (`Picker`/`PickerKind`), `domain/wizard.rs` (`ProviderWizard`/`WizardStep`/`WIZARD_KINDS`/`ADD_PROVIDER_LABEL`). Scan-only.

### [STRUCT-06] Split module-file convention: `mod.rs` in memory/sync/session vs `<name>.rs` everywhere else
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** `src/modules/memory/infrastructure/mod.rs`, `src/modules/sync/application/mod.rs`, `src/modules/session/domain/mod.rs` (10 `mod.rs` files) vs `src/modules.rs`, `src/modules/tui/infrastructure.rs`
- **Problem:** Three bounded contexts (`memory`, `sync`, `session`) declare their layers with the legacy `mod.rs` style, while the rest of the crate (`agent`, `provider`, `tools`, `tui`, `shared`, and the crate-root `src/modules.rs`) uses the `<name>.rs` + `<name>/` style. A reader navigating the tree must keep both mental models; new files land in whichever style the author last saw. This is a pure convention drift with no functional reason.
- **Evidence:**
```
src/modules.rs                          ← <name>.rs style (declares the 7 contexts)
src/modules/tui/infrastructure.rs       ← <name>.rs style
src/modules/memory/infrastructure/mod.rs ← legacy mod.rs style
src/modules/sync/application/mod.rs      ← legacy mod.rs style
src/modules/session/domain/mod.rs        ← legacy mod.rs style
```
- **Recommendation:** standardize on the dominant `<name>.rs` style (it matches `src/modules.rs` and the majority of the tree): rename each `<ctx>/<layer>/mod.rs` to `<ctx>/<layer>.rs`. Record the chosen convention as a one-line note in `CLAUDE.md`'s project-rules section so it stops drifting. Scan-only.

### [STRUCT-07] Oversized inline `#[cfg(test)]` modules inflate several files; extract to sibling test files
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/modules/agent/application/agent_loop.rs:362` (1,071 test lines over 360 prod), `src/modules/tools/application/registry.rs:96` (542 over 94), `src/modules/tools/infrastructure/sandbox.rs:352` (313 over 350), `src/modules/tui/infrastructure/view.rs` (331 over 85), `src/modules/tui/application/keymap.rs:947` (1,328 over 946)
- **Problem:** A raw `wc -l` census is misleading for this crate: `agent_loop.rs` reads as a 1,431-line file but its production surface is only ~360 lines — it is 75% inline tests. `registry.rs` is 85% tests; `view.rs` 80%. These production files are correctly sized; the problem is that the giant inline `mod tests` makes them *appear* to be the worst offenders and forces a reader to scroll past a thousand test lines to navigate. (Note: this is structural, not a request to delete tests — the ~10.8k test lines are a genuine strength.)
- **Evidence:**
```rust
// agent_loop.rs: production ends ~:360, then:
#[cfg(test)]
mod tests { /* 1,071 lines, lines 362-1431 */ }
```
- **Recommendation:** for files whose inline test module exceeds ~250 lines, declare `#[cfg(test)] mod tests;` and move the body to a sibling file (e.g. `agent_loop/tests.rs` once `agent_loop.rs` becomes `agent_loop/` per the `<name>.rs` convention, or a `#[path]`-attributed `agent_loop_tests.rs`). Private-item access is preserved because the child module still sees the parent's privates. This is the single highest-leverage way to make the census reflect real complexity. Scan-only.

### [STRUCT-08] `tui/infrastructure` (11 siblings) mixes runtime plumbing with rendering — group into subfolders
- **Severity:** Medium
- **Category:** folder-organization
- **Location:** `src/modules/tui/infrastructure/` (`bridge.rs`, `clipboard.rs`, `input.rs`, `layout.rs`, `markdown.rs`, `runtime.rs`, `terminal_guard.rs`, `text.rs`, `theme.rs`, `view.rs`, `widgets/`)
- **Problem:** Eleven sibling modules with no internal grouping conflate two clusters: the **runtime/IO plumbing** (`runtime`, `bridge`, `input`, `clipboard`, `terminal_guard`) and the **rendering** stack (`view`, `layout`, `markdown`, `text`, `theme`, `widgets/`). A reader cannot tell from the directory which files form the event-loop spine versus the draw path.
- **Evidence:** flat listing — `bridge.rs clipboard.rs input.rs layout.rs markdown.rs runtime.rs terminal_guard.rs text.rs theme.rs view.rs widgets/`.
- **Recommendation:** group into `tui/infrastructure/runtime/` (`runtime` per STRUCT-01, `bridge`, `input`, `clipboard`, `terminal_guard`) and `tui/infrastructure/render/` (`view`, `layout`, `markdown`, `text`, `theme`, `widgets/`). This composes with the STRUCT-01 `runtime/` split. Scan-only.

### [STRUCT-09] `shared/kernel` (9 siblings) mixes conversation types, provider primitives, and cross-cutting kernel — group the conversation cluster
- **Severity:** Low
- **Category:** folder-organization
- **Location:** `src/shared/kernel/` (`approval_mode.rs`, `completed_turn.rs`, `conversation.rs`, `error.rs`, `message.rs`, `provider.rs`, `role.rs`, `stream_event.rs`, `tool_call.rs`)
- **Problem:** Nine flat files blend three groups: the conversation/streaming types that break the agent↔provider cycle (`message`, `role`, `conversation`, `completed_turn`, `stream_event`), the provider primitives (`provider.rs` — 229 prod lines of `ProviderKind`/`AuthMethod`/`Effort`/`ProviderProfile`/`Credential`/`Secret`), and the genuinely cross-cutting kernel (`error`, `approval_mode`, `tool_call`). The grouping is invisible.
- **Evidence:** flat listing — `approval_mode.rs completed_turn.rs conversation.rs error.rs message.rs provider.rs role.rs stream_event.rs tool_call.rs`.
- **Recommendation:** group the conversation cluster under `shared/kernel/conversation/` (`message`, `role`, `conversation`, `completed_turn`, `stream_event`) and keep `error`, `approval_mode`, `tool_call`, `provider` at the kernel root. Optionally split the 6 provider primitives out of `provider.rs` if it grows. Lower priority — these files are individually small and well-named. Scan-only.

### [STRUCT-10] Split `markdown.rs` into parser and renderer
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/modules/tui/infrastructure/markdown.rs:44-520`
- **Problem:** ~521 production lines hold two phases of an inline markdown engine: the **parse** layer (`ParseCtx`, `Block`, `InlineAccum`, `FmtTag`, `BlockBuilder` and their ~200-line `impl ParseCtx`) and the **render-to-ratatui-lines** layer (`render`/`render_uncached` cache front, `render_block`, `wrap_spans`, `prepend_prefix`). They are separable along the AST boundary (`Block`).
- **Evidence:**
```rust
struct ParseCtx { /* ... */ }                         // :178  parse phase
impl ParseCtx { /* ~190 lines */ }                    // :210
fn render_block(block: &Block, width, out)            // :398  render phase
fn wrap_spans(runs, width, out)                       // :462
```
- **Recommendation:** promote to `markdown/` with `markdown/parse.rs` (the `ParseCtx`/`Block`/`InlineAccum`/`FmtTag`/`BlockBuilder` AST builder, exporting `Block`), `markdown/render.rs` (`render_block`/`wrap_spans`/`prepend_prefix`), and keep the cached `render`/`render_uncached` entry point in `markdown.rs`. Scan-only.

### [STRUCT-11] Test-only `characterization.rs` lives at the crate root, detached from the tools module it characterizes
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/characterization.rs:1-20` (declared at `src/main.rs:6`)
- **Problem:** This is a `#![cfg(test)]` file that snapshots the `tools` module's schema/confirmation surface, yet it sits at the crate root next to `main.rs`/`app.rs`/`modules.rs` — the only test-only file outside any module, and the only `#[path]`-style sibling of the composition root. A reader scanning the root sees a production-looking module that is actually a tools-specific test fixture.
- **Evidence:**
```rust
//! Characterization snapshot of the tool surface, captured against the pre-refactor code.
#![cfg(test)]
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
```
- **Recommendation:** relocate under the module it covers — `src/modules/tools/characterization.rs` (declared `#[cfg(test)] mod characterization;` in the tools module) — and move `src/snapshots/characterization.json` accordingly, or to a top-level `tests/` integration target. Scan-only.

### [STRUCT-12] `sync_service.rs` embeds the trust/config-diff DTOs that belong in their own file
- **Severity:** Low
- **Category:** file-size
- **Location:** `src/modules/sync/application/sync_service.rs:254-367`
- **Problem:** Alongside the `SyncService` use-case (init/push/pull/status), the file carries a self-contained trust sub-feature: `TrustView`/`TrustProvider`/`TrustSandbox`/`TrustEmbeddings` DTOs plus `risky_config_changes` (the security check that diffs an incoming config for credential/sandbox changes). This is a distinct concern — config-merge safety, not git sync orchestration — riding in the same module.
- **Evidence:**
```rust
struct TrustView { /* ... */ }            // :254
fn risky_config_changes(current, incoming) -> Vec<String> // :292
```
- **Recommendation:** move the `Trust*` DTOs + `risky_config_changes` (+ their tests) to `sync/application/trust.rs`, leaving `sync_service.rs` focused on the `SyncService` orchestration. Scan-only.

### [STRUCT-13] `tools/infrastructure/fs` (10 siblings) is acceptable by design — note only if it grows
- **Severity:** Low
- **Category:** folder-organization
- **Location:** `src/modules/tools/infrastructure/fs/` (`create_dir.rs`, `delete_dir.rs`, `delete_file.rs`, `edit_file.rs`, `list_dir.rs`, `move_path.rs`, `read_file.rs`, `run_command.rs`, `search.rs`, `write_file.rs`)
- **Problem:** Ten sibling files is the second-highest fan-out in the tree, but this is the **documented extension pattern** ("a new tool = one file under `tools/infrastructure/fs/`"), and each file is one cohesive `impl Tool`. The only mild outlier is `run_command.rs` (a process-exec tool) sitting under an `fs/` directory whose name implies filesystem tools.
- **Evidence:** `run_command.rs` is a shell-exec tool, not a filesystem op, yet lives in `fs/`.
- **Recommendation:** keep the one-file-per-tool layout as-is. If the directory grows past ~12–15, consider grouping (`fs/read/`, `fs/write/`, `fs/nav/`); meanwhile consider moving `run_command.rs` next to `exec.rs` under `tools/infrastructure/` (or a `tools/infrastructure/exec/` group) so `fs/` stays filesystem-only. Informational. Scan-only.

### [STRUCT-14] Provider DTO/SSE files are test-heavy, not oversized — no production split needed
- **Severity:** Low
- **Category:** file-size
- **Location:** `src/modules/provider/infrastructure/openai/sse.rs` (199 prod / 283 test), `src/modules/provider/infrastructure/anthropic/message_dto.rs` (199 / 190), `src/modules/provider/infrastructure/openai/provider.rs` (119 / 200)
- **Problem:** These appear in the >300-line band but their production surface is well within a single responsibility (one SSE decoder / one DTO mapping / one provider adapter each). They are flagged only to pre-empt a false "split these too" reaction to the raw census — the line count is test mass (see STRUCT-07), not production sprawl.
- **Evidence:** `openai/sse.rs` — 482 total, production ends ~line 199 at the `#[cfg(test)]` boundary.
- **Recommendation:** no production split. If extracting test modules per STRUCT-07, these benefit too. Scan-only.

## Prioritized modularization roadmap

| # | Action | Files | Severity | Effort | Payoff |
|---|---|---|---|---|---|
| 1 | Split `runtime.rs` into `runtime/{provider_swap,session_ops,distill,turn,render}.rs` | `tui/infrastructure/runtime.rs` | High | L | Dissolves the only god-file |
| 2 | Extract `Tui::run`'s effect dispatch into `apply_effect`/per-effect handlers | `tui/infrastructure/runtime.rs` | High | M | Tames the 410-line method |
| 3 | Split `config.rs` into `config/{raw,resolve,writers,cli}.rs` + facade | `shared/infra/config.rs` | Medium | M | Five concerns → five files |
| 4 | Split `view_state.rs` into named domain files | `tui/domain/view_state.rs` | Medium | M | Kills the grab-bag |
| 5 | Extract oversized inline `mod tests` to sibling files | `agent_loop.rs`, `registry.rs`, `keymap.rs`, `view.rs`, `sandbox.rs` | Medium | M | Census reflects real size |
| 6 | Split `keymap.rs` prod into `keymap/{editor_input,menu,submit,modals,wizard}.rs` | `tui/application/keymap.rs` | Medium | M | Five clusters → five files |
| 7 | Unify module-file convention on `<name>.rs` (rename 10 `mod.rs`) | memory/sync/session | Medium | S | One mental model |
| 8 | Group `tui/infrastructure` into `runtime/` + `render/` | `tui/infrastructure/*` | Medium | S | Navigability |
| 9 | Split `markdown.rs` into `markdown/{parse,render}.rs` | `tui/infrastructure/markdown.rs` | Medium | S | Parse vs render boundary |
| 10 | Extract `Trust*`/`risky_config_changes` to `sync/application/trust.rs` | `sync/application/sync_service.rs` | Low | S | Separates trust check |
| 11 | Group `shared/kernel/conversation/` cluster | `shared/kernel/*` | Low | S | Reveals grouping |
| 12 | Relocate `characterization.rs` under `modules/tools/` | `src/characterization.rs` | Low | S | Test fixture in place |
| 13 | Move `run_command.rs` out of `fs/` (process exec ≠ fs) | `tools/infrastructure/fs/run_command.rs` | Low | S | Honest dir naming |

## Strengths

- **Layering and module boundaries are sound.** The hexagonal `domain`/`application`/`infrastructure` split is consistent across all seven contexts, and the crate-root `src/modules.rs` + `<name>.rs` facades make the high-level tree easy to read — the issues here are intra-file sprawl, not a broken architecture.
- **Test mass is real and co-located.** ~10.8k of 28k lines are tests living next to the code they cover (the STRUCT-07 finding is about *file size optics*, not test quality) — every large file that looked alarming on a raw census turned out to be well-tested.
- **One-file-per-tool / one-adapter-per-provider extension patterns are honored** (`tools/infrastructure/fs/`, `provider/infrastructure/{openai,anthropic}/`), keeping the leaf adapters small and single-purpose even though their parent directories have high fan-out.
