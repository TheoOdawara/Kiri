# Audit — TUI Infrastructure

> Scope: everything under `src/modules/tui/infrastructure/` — `runtime.rs`, `markdown.rs`, `view.rs`, `bridge.rs`, `layout.rs`, `input.rs`, `text.rs`, `theme.rs`, `clipboard.rs`, `terminal_guard.rs`, `widgets.rs`, and `widgets/*` (`approval.rs`, `command_menu.rs`, `editor.rs`, `header.rs`, `hint_line.rs`, `meta_rule.rs`, `selection_overlay.rs`, `splash.rs`, `transcript_pane.rs`, `wizard.rs`)
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
This is a mature, carefully-written area: error-path discipline in the turn/distillation loops is genuinely senior (busy always resets, a draw failure breaks rather than `?`-propagates so cleanup runs), there are no runtime-reachable `unwrap`/`expect`/`panic` (every one is under `#[cfg(test)]`), and the pure render helpers (`cooling_fg`, `quench_fg`, `ramp`, `fit_context`, `click_to_cursor`) keep the clock out of the domain and are well unit-tested. The headline problems are structural, not correctness: `runtime.rs` is a 2072-line god-file that fuses six unrelated responsibilities; the `TranscriptItem::Notice` constructor is open-coded at 49 sites with no helper; `sync_push` constructs sibling-context infrastructure adapters inline and hardcodes the memory DB path (a layer/coupling leak); and the word-wrap algorithm is reimplemented three times with an already-divergent width metric. None of these are security or data-loss issues; they are maintainability and architecture-hygiene debt.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 5 | 7 |

## Findings

### [TUII-01] Split the 2072-line `runtime.rs` god-file by responsibility
- **Severity:** High
- **Category:** file-size
- **Location:** `src/modules/tui/infrastructure/runtime.rs:1` (whole file, 2072 lines); `ProviderSwap` `runtime.rs:79`; `Tui::run` `runtime.rs:301`; `drive_turn` `runtime.rs:813`; `flush_session`/`list_sessions`/`open_session` `runtime.rs:1148`; `sync_push` `runtime.rs:1352`; `drive_distillation` `runtime.rs:1443`
- **Problem:** One file carries the provider-swap state machine, the main event loop, the per-turn driver, session persistence orchestration, end-of-session distillation, and the `/sync` push — six distinct concerns. `Tui::run` alone spans `runtime.rs:301`–`705` with a ~260-line `for effect` match. This violates the "small & single-purpose files/functions" non-negotiable and makes the file hard to navigate, review, and test in isolation.
- **Evidence:**
```rust
// runtime.rs hosts, in one file:
pub struct ProviderSwap { client, secrets, providers, active, credential, thinking, effort }   // provider concern
pub struct Tui { agent_loop, sandbox, conversation, model, ... }                                // the front-end
async fn drive_turn(...) -> Result<()>                                                          // turn driver
async fn flush_session(...) / list_sessions(...) / open_session(...)                            // session I/O
async fn sync_push(config_path, model, terminal)                                                // sync push
async fn drive_distillation(...)                                                                // memory distill
```
- **Recommendation:** Introduce a `runtime/` submodule: `runtime/provider_swap.rs` (`ProviderSwap` + the four `apply_set_*`/`apply_save_provider`), `runtime/turn.rs` (`drive_turn`, `Step`, `forces_draw`, `on_turn_end`, `turn_produced_nothing`, `engine_msg`), `runtime/session_io.rs` (`flush_session`, `list_sessions`, `open_session`, `rebuild_transcript`, `short_timestamp`), `runtime/distill.rs` (`drive_distillation`, `should_distill`, `is_ctrl_c`, `DistillStep`), `runtime/sync.rs` (`sync_push`), leaving `runtime.rs` with `Tui`, `run`, `draw_and_copy`, and the clipboard/cursor glue. Scan-only: do not implement.

### [TUII-02] No `Notice` helper — the transcript-notice constructor is open-coded 49 times
- **Severity:** Medium
- **Category:** duplication
- **Location:** 49 call sites, e.g. `runtime.rs:359`, `runtime.rs:482`, `runtime.rs:533`, `runtime.rs:973`, `runtime.rs:1015`, `runtime.rs:1049`, `runtime.rs:1178`, `runtime.rs:1360`, `runtime.rs:1466`
- **Problem:** Every status/error message repeats `model.transcript.push(TranscriptItem::Notice(NoticeLevel::Error | Info, format!(...)))`. The "persist failed → push error notice" block alone recurs verbatim across `apply_set_model`/`apply_set_effort`/`apply_set_provider`/`apply_save_provider`. This is a large, mechanical duplication that bloats every function and makes a future change (e.g. adding a notice timestamp or de-dup) a 49-site edit.
- **Evidence:**
```rust
// runtime.rs:973 — one of 49 near-identical sites
model.transcript.push(TranscriptItem::Notice(
    NoticeLevel::Info,
    format!("modelo: {model_id}"),
));
// ...and the repeated persist-failure block:
if let Err(error) = config::persist_active_model(config_path, &provider_swap.active, &model_id) {
    model.transcript.push(TranscriptItem::Notice(
        NoticeLevel::Error,
        format!("não persistiu o modelo: {error:#}"),
    ));
}
```
- **Recommendation:** Add `Model::notify_info(&mut self, impl Into<String>)` / `notify_error(...)` (or a single `notify(level, msg)`), and a `persist_or_notice(result, model, context)` helper for the persist-then-report pattern. Collapses ~49 sites to one-liners. Scan-only.

### [TUII-03] `sync_push` constructs sibling-context adapters inline and hardcodes the memory DB path
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/tui/infrastructure/runtime.rs:1367`–`1390`; imports `runtime.rs:25` (`SqliteSharedMemory`) and `runtime.rs:32` (`GitCli`)
- **Problem:** The TUI infrastructure layer reaches directly into the `memory` and `sync` *infrastructure* adapters: it `new`s a `SqliteSharedMemory`, calls `.init()` (SQLite DDL I/O), and `new`s a `GitCli`, and it recomputes the shared-memory DB location `global_dir.join("memory").join("shared.db")` — duplicating the `memory` module's own path convention (`~/.kiri/memory/shared.db`) inside the front-end. The `Tui` is already handed `memory: Arc<dyn MemoryPort>` (`runtime.rs:254`) by the composition root, proving the pattern; the sync path should be wired the same way rather than hand-assembled here. This puts adapter construction and another context's path layout in the wrong layer.
- **Evidence:**
```rust
let shared_db = global_dir.join("memory").join("shared.db");   // memory's path convention, duplicated in tui
let memory = match SqliteSharedMemory::new(shared_db) {        // memory adapter built in the tui layer
    Ok(store) => match store.init().await { ... }
    ...
};
let git = GitCli;                                              // sync adapter built in the tui layer
let service = SyncService::new(&git, global_dir, config_path.to_path_buf(), &memory);
```
- **Recommendation:** Wire the `SharedStore` (or a `SyncService` factory/closure) at `app::wire` and inject it into `Tui`, so `sync_push` calls a pre-built port and neither the path nor the concrete adapters leak into the front-end. Scan-only.

### [TUII-04] Word-wrap is reimplemented three times, with a divergent width metric
- **Severity:** Medium
- **Category:** duplication
- **Location:** `markdown.rs:462` (`wrap_spans`); `widgets/transcript_pane.rs:368`/`383` (`hard_wrap`/`wrap_line`); `widgets/editor.rs:136` (`logical_rows`)
- **Problem:** Three greedy word-wrap implementations encode the same algorithm. They have already diverged on the width metric: `transcript_pane::wrap_line` measures with `display_width(word)` (`transcript_pane.rs:386`) while `editor::logical_rows` measures with `word.chars().count()` (`editor.rs:144`). The editor's comment (`editor.rs:122`–`125`) acknowledges the undercount for wide glyphs — i.e. the input-box height computed by `logical_rows` can disagree with what the transcript wrap would produce. Three copies of one algorithm with subtle drift is a maintenance hazard.
- **Evidence:**
```rust
// editor.rs:144 — counts scalar chars
let wlen = word.chars().count();
// transcript_pane.rs:386 — counts display cells
let word_cols = display_width(word);
```
- **Recommendation:** Extract one greedy word-wrap primitive in `text.rs` parameterized by a width function and a per-element callback (so the span-based, string-based, and count-only callers all reuse it). At minimum, make all three measure with `display_width`. Scan-only.

### [TUII-05] Pervasive `#[allow(clippy::too_many_arguments)]` — the runtime I/O handles want a context struct
- **Severity:** Medium
- **Category:** architecture
- **Location:** `runtime.rs:260` (`Tui::new`, 10 args), `runtime.rs:813` (`drive_turn`, 11 args), `runtime.rs:1266` (`open_session`, 8 args), `runtime.rs:1442` (`drive_distillation`, 8 args)
- **Problem:** Four functions suppress the argument-count lint. The same UI-driver handles — `terminal`, `events`, `ticker` (and for the turn path also `bridge`, `engine_rx`, `cancel`, `pending_reply`) — are threaded by hand through `drive_turn` and `drive_distillation`. Silencing the lint rather than addressing it is the smell the lint exists to catch, and the repeated handle-threading is itself duplication at every call site (e.g. `runtime.rs:370`–`382`, `runtime.rs:464`–`474`, `runtime.rs:615`–`627`).
- **Evidence:**
```rust
#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    agent_loop: &AgentLoop, conversation: &mut Conversation, sandbox: &dyn Sandbox,
    bridge: &mut Bridge, model: &mut Model, engine_rx: &mut mpsc::UnboundedReceiver<EngineMsg>,
    cancel: &CancelToken, pending_reply: &mut Option<oneshot::Sender<Approval>>,
    terminal: &mut DefaultTerminal, events: &mut EventStream, ticker: &mut Interval,
) -> Result<()>
```
- **Recommendation:** Bundle the live UI handles into a `UiDriver { terminal, events, ticker }` (and an engine-handles struct for `bridge`/`engine_rx`/`cancel`/`pending_reply`), passed as `&mut`. Removes the `allow`s and shrinks every call site. Scan-only.

### [TUII-06] The selected-option row pattern is duplicated across three widgets
- **Severity:** Medium
- **Category:** duplication
- **Location:** `widgets/approval.rs:49`–`57`, `widgets/command_menu.rs:24`–`39`, `widgets/wizard.rs:50`–`58`
- **Problem:** The "selected row carries `❯ ` + accent, others carry `  ` + dim" idiom is open-coded in all three list/stanza widgets. It is the same visual contract three times; a change to the caret glyph or the selected style touches three files.
- **Evidence:**
```rust
// repeated verbatim in approval.rs, command_menu.rs, wizard.rs
let (marker, style) = if i == selected {
    ("❯ ", theme::accent())
} else {
    ("  ", theme::dim())
};
```
- **Recommendation:** Add a shared `widgets::option_marker(selected: bool) -> (&'static str, Style)` (or a `selectable_row` line builder) in a small `widgets` helper and call it from all three. Scan-only.

### [TUII-07] `markdown::ParseCtx::start` takes a `&mut Vec<Block>` it never uses
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/modules/tui/infrastructure/markdown.rs:224` (signature), `markdown.rs:278` (`let _ = blocks;`)
- **Problem:** `start` accepts `blocks: &mut Vec<Block>` but only `end`/`push_block` ever push to it; the parameter is silenced with a trailing `let _ = blocks;`. A parameter that exists only to be discarded is dead surface and misleads the reader into thinking `start` mutates the block list.
- **Evidence:**
```rust
fn start(&mut self, tag: Tag, blocks: &mut Vec<Block>) {
    match tag { /* ... never touches `blocks` ... */ }
    let _ = blocks;   // markdown.rs:278
}
// caller: ctx.start(tag, &mut blocks);
```
- **Recommendation:** Drop the `blocks` parameter from `start` and the argument at the call site. Scan-only.

### [TUII-08] Two `let _ = draw_and_copy(...)` discards lack the contract-required justification comment
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/tui/infrastructure/runtime.rs:1365` (`sync_push`), `runtime.rs:1473` (`drive_distillation`)
- **Problem:** The project contract states a bare `let _ = <fallible>` is a defect unless it carries a one-line comment justifying why the failure is safe to ignore. These two draw-error discards have no inline justification (the rationale lives only in the function docstring). Every other `let _ =` in the file (`runtime.rs:160`, `:221`, `:326`, `:872`, `:1203`) is correctly annotated, so these two stand out as the gap.
- **Evidence:**
```rust
// runtime.rs:1364 — sync_push, no inline justification on the discard
model.render_at = Some(Instant::now());
let _ = draw_and_copy(terminal, model);
// runtime.rs:1472 — drive_distillation, same
model.render_at = Some(started);
let _ = draw_and_copy(terminal, model);
```
- **Recommendation:** Add a one-line comment at each site (e.g. "best-effort pre-op repaint; the next loop iteration redraws, so a failure here is non-fatal"), matching the discipline of the other discards. Scan-only.

### [TUII-09] Modal priority order is encoded twice in `view.rs`
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/tui/infrastructure/view.rs:28`–`36` (render dispatch) and `view.rs:72`–`82` (`frame_regions` `box_h`)
- **Problem:** The precedence "pending_plan → pending_approval → picker → wizard" is written as an `if/else if` chain twice: once to decide which stanza to render, once to size its reserved region. The two must stay in lockstep — if one ever orders the modals differently, the box is sized for one modal and rendered for another. The coupling is implicit.
- **Evidence:**
```rust
// view.rs:28 (render) and view.rs:72 (sizing) — the same four-way precedence, duplicated
if let Some(plan) = &model.pending_plan { ... }
else if let Some(pending) = &model.pending_approval { ... }
else if let Some(picker) = &model.picker { ... }
else if let Some(provider_wizard) = &model.wizard { ... }
```
- **Recommendation:** Resolve the active modal once (e.g. a `model.active_modal() -> Option<ModalRef>` in the domain) and drive both the sizing and the render from that single value. Scan-only.

### [TUII-10] `short_timestamp` slices with a magic `16`
- **Severity:** Low
- **Category:** magic
- **Location:** `src/modules/tui/infrastructure/runtime.rs:1212`–`1214`
- **Problem:** The RFC3339 trim uses `raw.get(..16)` with no named constant; `16` is the length of `YYYY-MM-DD HH:MM` but a reader must count characters to know that. The doc comment explains intent but the literal itself is unexplained in code.
- **Evidence:**
```rust
fn short_timestamp(raw: &str) -> String {
    raw.get(..16).unwrap_or(raw).replace('T', " ")
}
```
- **Recommendation:** Name it, e.g. `const MINUTE_PRECISION_LEN: usize = 16; // "YYYY-MM-DD HH:MM"`. Scan-only.

### [TUII-11] Clipboard image-encode failure and empty paste are silent no-ops on a direct user intent
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/tui/infrastructure/clipboard.rs:51`–`69` (`encode_png_data_url`, `.ok()?` at `:61`–`:62`); `runtime.rs:763`–`775` (`paste_from_clipboard`, `ClipboardContent::Empty => {}` at `:773`)
- **Problem:** When the clipboard holds an image that fails PNG encoding, `encode_png_data_url` swallows the error via `.ok()?` and returns `None`; `read()` then falls through to text and, if there is none, yields `Empty`, which `paste_from_clipboard` handles as a do-nothing. So Ctrl+V on a genuinely-failing image produces no attachment and no feedback. The contract calls out "no silent no-ops on user intent" and "surface, don't swallow"; an empty clipboard is benign, but a real encode failure on a present image should be visible.
- **Evidence:**
```rust
let mut writer = encoder.write_header().ok()?;            // clipboard.rs:61 — failure → None, no signal
writer.write_image_data(image.bytes.as_ref()).ok()?;     // clipboard.rs:62
// runtime.rs:773
ClipboardContent::Empty => {}                            // nothing surfaced
```
- **Recommendation:** Distinguish "nothing in the clipboard" from "had an image but encoding failed" (e.g. a `ClipboardContent::Unreadable` variant or a logged notice for the encode-failure branch) so a real failure surfaces a Notice. Scan-only.

### [TUII-12] `markdown::render` clones the whole markdown string into the cache key on every call, including hits
- **Severity:** Low
- **Category:** performance
- **Location:** `src/modules/tui/infrastructure/markdown.rs:44`–`48`
- **Problem:** The memoization key is `(markdown.to_string(), base, width)`, built *before* the lookup, so even a cache hit allocates a full copy of the item's source text to hash it. The whole finalized transcript is re-rendered every frame (`transcript_pane::render` → `markdown::render` per item), so the hot idle/stream path re-clones every transcript item's text each frame. The parse is avoided, but the per-frame string cloning is not. The module comment frames the clone as "per uncached entry," which understates that it also happens on every hit.
- **Evidence:**
```rust
pub fn render(markdown: &str, base: Style, width: usize) -> Vec<Line<'static>> {
    let key: CacheKey = (markdown.to_string(), base, width);   // allocates on every call, hit or miss
    if let Some(hit) = RENDER_CACHE.with(|c| c.borrow().get(&key).cloned()) { return hit; }
    ...
}
```
- **Recommendation:** Key on a cheap fingerprint kept collision-safe (e.g. `(len, blake3/fxhash of the text, base, width)`, or borrow the `&str` for the lookup via a `Borrow`-friendly key) so a hit needs no full-string allocation. Measure first per the perf guidance. Scan-only.

### [TUII-13] Spinner-glyph selection is duplicated between `theme::gate` and `meta_rule`
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/tui/infrastructure/theme.rs:131` and `widgets/meta_rule.rs:25`
- **Problem:** `SPINNER[frame % SPINNER.len()]` is computed in two places (and the frame index itself in `runtime::spinner_frame`). Minor, but it is the same wrap-into-the-glyph-table expression copied; a single accessor would centralize it.
- **Evidence:**
```rust
// theme.rs:131
GateState::Busy(frame) => (SPINNER[frame % SPINNER.len()], Style::default().fg(HIGHLIGHT)),
// meta_rule.rs:25
let glyph = theme::SPINNER[model.status.spinner_frame % theme::SPINNER.len()];
```
- **Recommendation:** Add `theme::spinner_glyph(frame: usize) -> char` and call it from both. Scan-only.

## Strengths
- **Exemplary error-path discipline in the turn/distillation loops:** `drive_turn` (`runtime.rs:932`–`936`) deliberately `break`s on a draw failure instead of `?`-propagating so `on_turn_end` always runs and `model.busy` always resets; `drive_distillation` resets `busy` unconditionally after its loop (`runtime.rs:1503`). The per-turn lifecycle-flag invariant is honored on every exit path.
- **No runtime-reachable panics:** every `unwrap`/`expect`/`panic!` in the area is inside `#[cfg(test)]`; production paths use `unwrap_or`/`unwrap_or_else`/`ok_or_else` with sane fallbacks (e.g. `runtime.rs:585`, `clipboard.rs:21`, `selection_overlay` bounds clamping).
- **Clock kept out of pure logic, and tested as such:** `cooling_fg`, `quench_fg`, `theme::ramp`, `spinner_frame`, `splash::row_progress`, `fit_context`, and `click_to_cursor` are pure functions taking instants/widths as inputs, each with focused unit tests — making the animation and layout math verifiable without a terminal.
- **`selection_overlay` paint/scrape share one geometry resolver** (`resolve`/`row_span`) so the highlight and the copied text can never disagree, and both are explicitly panic-safe against stale out-of-bounds coordinates (`selection_overlay.rs:228`, `:296`).
