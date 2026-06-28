# Audit — TUI core (domain + application)

> Scope: `src/modules/tui.rs`, `src/modules/tui/application/` (`keymap.rs`, `update.rs`, `command.rs`, `effect.rs`, `msg.rs`) and `src/modules/tui/domain/` (`view_state.rs`, `model.rs`, `command_menu.rs`, `transcript.rs`) — every file read in full.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The TUI core is a clean, well-tested Elm-style reducer: the message/effect split is disciplined, secrets are handled exemplarily (staged `Secret`, `Debug` redaction, `Drop` zeroize), and there are **no runtime-reachable `unwrap`/`expect`/`panic` and no I/O in the reducer** — every index is clamped or guarded. The headline issues are all maintainability/consistency, not correctness: the `domain` layer couples directly to the `ratatui` and `tui_textarea` rendering crates (a hexagonal-purity breach), two oversized grab-bag files (`view_state.rs`, `keymap.rs`), and several pieces of duplicated/divergent logic — three copies of wrapping cursor arithmetic, three parallel command catalogs that have already drifted, and inconsistent modal-navigation/digit-handling styles across the four modal handlers. No Critical issues; one High (the architecture coupling).

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 7 | 4 |

## Findings

### [TUIC-01] Domain layer depends on `ratatui` and `tui_textarea` rendering crates
- **Severity:** High
- **Category:** architecture
- **Location:** `src/modules/tui/domain/view_state.rs:1`, `src/modules/tui/domain/view_state.rs:2`, `src/modules/tui/domain/view_state.rs:22-24`, `src/modules/tui/domain/view_state.rs:133`, `src/modules/tui/domain/view_state.rs:141`
- **Problem:** The project contract defines `domain/` as "pure data/rules, no I/O" and the hexagonal rule keeps frameworks out of the domain. `view_state.rs` is the **only** domain file in the whole codebase that imports UI framework crates (verified by grep across `src/modules/*/domain`): `InputBuffer` embeds a stateful `tui_textarea::TextArea` (which owns cursor, viewport, soft-wrap and undo/redo history) and `set_styles`/`widget` expose `ratatui::style::Style` and `&TextArea`. The domain is therefore not pure data — it carries view/widget state and a rendering type, coupling the innermost layer to two third-party UI libraries. It is a deliberate, documented tradeoff (the doc-comment says it "confines the widget type to this module"), but it still breaches the stated invariant and should be ratified rather than implicit.
- **Evidence:**
```rust
use ratatui::style::Style;
use tui_textarea::{CursorMove, Input, TextArea, WrapMode};
// ...
pub struct InputBuffer {
    area: TextArea<'static>,
}
// ...
pub fn set_styles(&mut self, base: Style, cursor: Style, selection: Style) { /* ... */ }
pub fn widget(&self) -> &TextArea<'static> { &self.area }
```
- **Recommendation:** Either (a) record an ADR accepting `InputBuffer` as a domain-owned widget wrapper (the pragmatic path, since the widget is the editor's source of truth), or (b) relocate the `TextArea`-backed `InputBuffer` to `tui/infrastructure` (or `application`) behind a pure-data domain shadow, keeping `domain/` free of `ratatui`/`tui_textarea`. Do not implement here — flag for an architecture decision.

### [TUIC-02] `view_state.rs` is a 767-line grab-bag of ~10 unrelated domain types
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/modules/tui/domain/view_state.rs:1-767`
- **Problem:** A single file holds `ImageAttachment`, `InputBuffer`, `Granularity`, `SelectionState`, `ScreenSelection`, `History`, `Scroll`, `PendingApproval`, `PLAN_OPTIONS`/`PendingPlan`, `PickerKind`, `ProviderWizard`/`WizardStep`/`WIZARD_KINDS`/`ADD_PROVIDER_LABEL`, and `Picker` — at least ten independent concerns (text editing, mouse selection, history recall, scrollback, three different modal states, the provider wizard). The "single responsibility per file" rule is clearly exceeded; the file name `view_state` is a catch-all that hides what lives inside.
- **Evidence:**
```rust
pub struct InputBuffer { /* line 22 */ }
pub struct ScreenSelection { /* line 174 */ }
pub struct History { /* line 218 */ }
pub struct PendingApproval { /* line 302 */ }
pub struct ProviderWizard { /* line 394 */ }
pub struct Picker { /* line 578 */ }
```
- **Recommendation:** Split into a `view_state/` submodule by responsibility: `input_buffer.rs` (+`ImageAttachment`), `selection.rs` (`Granularity`/`SelectionState`/`ScreenSelection`), `history.rs`, `scroll.rs`, `approval.rs` (`PendingApproval`/`PendingPlan`/option constants), `picker.rs` (`Picker`/`PickerKind`), `wizard.rs` (`ProviderWizard`/`WizardStep`/`WIZARD_KINDS`/`ADD_PROVIDER_LABEL`). Scan-only — do not implement.

### [TUIC-03] `keymap.rs` mixes many responsibilities in one ~945-line module (2274 with tests)
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/modules/tui/application/keymap.rs:32-945`
- **Problem:** A single application file owns: top-level key dispatch (`on_key`), mouse gestures (`on_mouse`/`click_granularity`), key→widget mapping (`to_input`), the slash-command preview (`sync_menu`/`on_menu_key`/`complete_command`), submit + full command routing (`submit` + three `open_*_picker` helpers), and four modal handlers (`on_approval_key`, `on_plan_key`, `on_picker_key`, `on_wizard_key`/`advance_wizard`). That is many distinct reasons to change in one file; the inline test module (lines 947-2274, ~1329 lines) is larger than the code it covers, further inflating the file.
- **Evidence:**
```rust
pub fn on_key(model: &mut Model, key: KeyPress) -> Vec<Effect> { /* dispatch + chords */ }
pub fn on_mouse(/* ... */) -> Vec<Effect> { /* selection gestures */ }
fn submit(model: &mut Model) -> Vec<Effect> { /* command routing */ }
fn on_wizard_key(/* ... */) -> Vec<Effect> { /* provider wizard */ }
```
- **Recommendation:** Extract a `keymap/` submodule: `editor.rs` (`on_key` chord/edit core + `to_input`), `mouse.rs` (`on_mouse`/`click_granularity`), `menu.rs` (`sync_menu`/`on_menu_key`/`complete_command`), `submit.rs` (`submit` + `open_*_picker`), `modals.rs` or one file each for `approval.rs`/`plan.rs`/`picker.rs`/`wizard.rs`. Move each handler's tests alongside it. Scan-only.

### [TUIC-04] Three near-identical copies of wrapping `rem_euclid` cursor movement
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/tui/domain/command_menu.rs:138-147`, `src/modules/tui/domain/view_state.rs:607-615`, `src/modules/tui/domain/view_state.rs:549-555`
- **Problem:** `CommandMenu::move_cursor`, `Picker::move_cursor`, and `ProviderWizard::move_kind` each re-implement the same "advance index by delta, wrap with `rem_euclid` over a length, guard empty" arithmetic. Three copies of the same logic drift independently and must each be tested separately.
- **Evidence:**
```rust
// command_menu.rs
let len = self.filtered.len() as i32;
let mut next = self.selected as i32 + delta;
next = next.rem_euclid(len);
self.selected = next as usize;
// view_state.rs Picker
let len = self.options.len() as i32;
self.selected = (self.selected as i32 + delta).rem_euclid(len) as usize;
// view_state.rs ProviderWizard
let len = WIZARD_KINDS.len() as i32;
self.kind_selected = (self.kind_selected as i32 + delta).rem_euclid(len) as usize;
```
- **Recommendation:** Extract a single pure helper (e.g. `fn wrapping_step(index: usize, delta: i32, len: usize) -> usize` in `domain`) and have all three call it. Scan-only.

### [TUIC-05] Modal navigation wraps in some handlers but clamps in others
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** `src/modules/tui/application/keymap.rs:577-586` (approval), `src/modules/tui/application/keymap.rs:647-656` (plan), `src/modules/tui/application/keymap.rs:702-711` (picker), `src/modules/tui/application/keymap.rs:807-808` (wizard kind), `src/modules/tui/application/keymap.rs:375-386` (menu)
- **Problem:** The picker, the wizard kind-chooser, and the command menu wrap the highlight (`rem_euclid`), while the approval box and the plan box clamp it (`saturating_sub` / `.min(len-1)`). The four modal lists therefore behave differently under Up/Down for no stated reason — pressing Up at the top wraps in three modals and sticks in two. This is both a UX inconsistency and divergent code style for the same gesture.
- **Evidence:**
```rust
// approval (clamp)
Key::Up => { pending.selected = pending.selected.saturating_sub(1); return vec![]; }
Key::Down => { pending.selected = (pending.selected + 1).min(APPROVAL_OPTIONS.len() - 1); return vec![]; }
// picker (wrap)
Key::Up => { picker.move_cursor(-1); return vec![]; }
Key::Down => { picker.move_cursor(1); return vec![]; }
```
- **Recommendation:** Pick one navigation semantics for all single-choice modals (wrapping is already the majority) and route approval/plan through the same shared helper from TUIC-04. Scan-only.

### [TUIC-06] Three parallel command catalogs, already drifted in their blurbs
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/tui/application/command.rs:46-61` (parse aliases), `src/modules/tui/application/command.rs:66-86` (`help_text`), `src/modules/tui/domain/command_menu.rs:17-83` (`COMMANDS`)
- **Problem:** The command set is encoded three times — the parser's alias arms, the `help_text()` lines, and the `COMMANDS` catalog (name + aliases + blurb). Only the canonical names are test-locked (`catalog_matches_parse`); the aliases and the human descriptions are not. They have already diverged: `/plan` reads "modo plan (só leitura; planeja e executa após aprovação)" in `help_text` but "modo plan (planeja e executa após aprovação)" in `COMMANDS`; `/auto` reads "executa **tudo** sem pedir aprovação" vs "executa sem pedir aprovação". A reader gets different help depending on whether they typed `/help` or opened the live menu.
- **Evidence:**
```rust
// command.rs help_text()
"  /plan          modo plan (só leitura; planeja e executa após aprovação)",
"  /auto          modo auto (executa tudo sem pedir aprovação)",
// command_menu.rs COMMANDS
CommandSpec { name: "/plan",  aliases: &[], blurb: "modo plan (planeja e executa após aprovação)" },
CommandSpec { name: "/auto",  aliases: &[], blurb: "modo auto (executa sem pedir aprovação)" },
```
- **Recommendation:** Make `COMMANDS` the single source: derive `help_text()` by formatting the catalog, and add a test asserting the alias set in `COMMANDS` matches the alias arms in `parse` (not just canonical names). Scan-only.

### [TUIC-07] Hardcoded per-digit / per-index arms couple silently to option-array length and order
- **Severity:** Medium
- **Category:** magic
- **Location:** `src/modules/tui/application/keymap.rs:594-596` (approval digits), `src/modules/tui/application/keymap.rs:666-672` (plan digits), `src/modules/tui/application/keymap.rs:677-693` (plan index→effect), `src/modules/tui/application/keymap.rs:611-617` (approval index→decision)
- **Problem:** `on_approval_key` hardcodes `'1'→0, '2'→1, '3'→2` (assuming exactly 3 options) and maps `Choice::Option(0/1/_)` to `Approved`/`ApprovedAuto`/`Declined` by position; `on_plan_key` hardcodes `'1'..'4'→0..3` and a positional `0/1/2/_` → effect mapping. The picker handler, by contrast, generalizes with `c.is_ascii_digit()` + a bounds check. If `APPROVAL_OPTIONS` or `PLAN_OPTIONS` ever gains/loses an entry or is reordered, the digit shortcuts and the index→semantics mapping drift silently with no compile error — exactly the "no magic / implicit coupling" the contract warns against.
- **Evidence:**
```rust
Key::Char('1') => Some(Choice::Option(0)),
Key::Char('2') => Some(Choice::Option(1)),
Key::Char('3') => Some(Choice::Option(2)),
// ...later, positional:
Choice::Option(0) => (Approval::Approved, false),
Choice::Option(1) => (Approval::ApprovedAuto, true),
Choice::Option(_) => (Approval::Declined, false),
```
- **Recommendation:** Drive digit selection from `options.len()` (as the picker does) and replace the positional index→semantics mapping with named option enums or an explicit table keyed to each option, so reordering the array cannot desync the behaviour. Scan-only.

### [TUIC-08] Mouse-wheel scroll uses an unnamed `3` that disagrees with `SCROLL_STEP = 5`
- **Severity:** Medium
- **Category:** magic
- **Location:** `src/modules/tui/application/update.rs:108`, `src/modules/tui/application/update.rs:113`, `src/modules/tui/application/keymap.rs:18`
- **Problem:** `Msg::ScrollUp`/`ScrollDown` scroll by a bare literal `3`, while keyboard scrolling uses the named `SCROLL_STEP = 5` / `SCROLL_PAGE = 20` constants in `keymap.rs`. The magic `3` has no name explaining why a wheel notch differs from a `PageUp` step, and the two scroll surfaces are inconsistent.
- **Evidence:**
```rust
Msg::ScrollUp => { model.clear_screen_selection(); model.scroll.up(3); Vec::new() }
Msg::ScrollDown => { model.clear_screen_selection(); model.scroll.down(3); Vec::new() }
```
- **Recommendation:** Introduce a named `WHEEL_STEP` constant (shared with `keymap`'s scroll constants in one place) and use it in both arms; decide deliberately whether wheel and keyboard steps should match. Scan-only.

### [TUIC-09] `Model` aggregates ~30 fields spanning four distinct concerns
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/tui/domain/model.rs:67-140`
- **Problem:** The single `Model` struct mixes core conversation state (`transcript`, `input`, `history`, `scroll`, `status`), modal state (`pending_approval`, `pending_plan`, `picker`, `wizard`, `command_menu`, plus the `models`/`providers`/`session_ids`/`pending_credential` they need), animation/timing state (`motion`, `render_at`, `stream_landings`, `turn_settled_at`, `opened_at`, `last_ctrl_c`, `last_esc`, `last_event_at`), and selection state (`selection`, `last_click`). Roughly thirty fields make the model hard to reason about and easy to leave a stale field on a transition (e.g. multiple timing instants that must each be reset). Some of this is inherent to the Elm pattern, but the timing and selection clusters are cleanly separable.
- **Evidence:**
```rust
pub last_ctrl_c: Option<Instant>,
pub last_esc: Option<Instant>,
pub motion: Motion,
pub render_at: Option<Instant>,
pub stream_landings: Vec<Instant>,
pub turn_settled_at: Option<Instant>,
pub opened_at: Option<Instant>,
pub selection: Option<ScreenSelection>,
pub last_click: Option<(Instant, (u16, u16), u8)>,
pub last_event_at: Option<Instant>,
```
- **Recommendation:** Group the animation/timing fields into a `Timeline`/`Animation` sub-struct and the mouse-selection fields (`selection`, `last_click`, `last_event_at`) into a `Selection` sub-struct, leaving `Model` with a handful of high-level members. Scan-only.

### [TUIC-10] `Effort::ALL.get(index)...unwrap_or_default()` swallows an out-of-range path without justification
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/tui/application/keymap.rs:748`
- **Problem:** The contract requires that any `.unwrap_or_default()` which could hide a real failure carry a one-line justification. Here the index is bounded by the picker (built from `Effort::ALL`), so out-of-range is unreachable — but that reasoning is not written down, and a future change to how the effort picker is populated could let a silent `Effort::default()` mask a mismatch.
- **Evidence:**
```rust
PickerKind::Effort => {
    let effort = Effort::ALL.get(index).copied().unwrap_or_default();
    vec![Effect::SetEffort(effort)]
}
```
- **Recommendation:** Add a one-line comment stating the index is guaranteed in range by the picker construction, or mirror the `Models`/`Sessions` arms (`match ...get(index) { Some => ..., None => vec![] }`) so an out-of-range index is a no-op rather than a silent default. Scan-only.

### [TUIC-11] Ctrl+C handling diverges across the four modal handlers with no explaining comment
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/tui/application/keymap.rs:611-617` (approval Abort), `src/modules/tui/application/keymap.rs:659-663` (plan), `src/modules/tui/application/keymap.rs:714-718` (picker), `src/modules/tui/application/keymap.rs:778-782` (wizard)
- **Problem:** Plan, picker, and wizard all handle Ctrl+C by setting `model.should_quit = true` and emitting `Effect::Quit`. The approval handler instead emits `Effect::AnswerApproval(Approval::Aborted)` and leaves `should_quit` false. This is almost certainly correct (the engine is blocked on the approval reply channel, so it must be answered rather than a bare `Quit` that would strand the engine future), but nothing in the code says so — a reader comparing the four handlers sees an unexplained asymmetry and might "fix" it into a hang.
- **Evidence:**
```rust
// approval: no should_quit, answers the channel
Choice::Abort => (Approval::Aborted, false),
// plan/picker/wizard: direct quit
model.should_quit = true;
return vec![Effect::Quit];
```
- **Recommendation:** Add a one-line `why` comment on the approval-abort path noting that the pending reply channel must be answered (so a bare `Quit` would hang the engine), making the divergence intentional and obvious. Scan-only.

### [TUIC-12] `view_state.rs` filename understates its contents
- **Severity:** Low
- **Category:** naming
- **Location:** `src/modules/tui/domain/view_state.rs:1`
- **Problem:** "view state" reads as render-time presentation data, but the file actually owns the editor buffer, prompt history, mouse selection, scrollback, three modal states, and the provider wizard — i.e. most of the `Model`'s behavioural sub-state, not view state. The misleading name compounds the discoverability problem in TUIC-02.
- **Evidence:**
```rust
pub struct InputBuffer { /* editor */ }
pub struct History { /* prompt recall */ }
pub struct ProviderWizard { /* multi-step form */ }
```
- **Recommendation:** Resolve together with the TUIC-02 split (per-concern files under a `view_state/` or renamed `state/` module); a name like `state` or per-type modules conveys the contents better. Scan-only.

## Strengths
- **Exemplary secret handling:** the typed API key never rides in a `Debug`-printable `Effect` (`effect.rs:48-59`), `ProviderWizard` has a manual `Debug` that masks the key (`view_state.rs:409-422`) and a `Drop` that zeroizes it (`view_state.rs:569-573`), and the key is staged as a `Secret` via `mem::take` at finalize (`keymap.rs:940`). The paste-into-wizard regression is explicitly guarded with a test (`update.rs:213-231`).
- **No runtime panics and a pure reducer:** every array index in the reducer is clamped (`ProviderWizard::kind` `.min(len-1)`, `Picker::new` `saturating_sub`) or guarded, digit parsing uses `unwrap_or`, and there is no `unwrap`/`expect`/`panic!` or any I/O outside `#[cfg(test)]` — the reducer is fully unit-testable, as designed.
- **Strong, behaviour-focused test coverage** for the editor chords, mouse multi-click escalation, the modal state machines, the onboarding gate, and the menu/parser sync invariant (`command_menu.rs:235-245`).
