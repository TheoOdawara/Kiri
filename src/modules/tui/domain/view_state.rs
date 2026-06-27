use ratatui::style::Style;
use tui_textarea::{CursorMove, Input, TextArea, WrapMode};
use zeroize::Zeroize;

use crate::shared::kernel::provider::ProviderKind;

/// A pasted image staged for the next prompt: its data URL (base64 PNG, ready for the provider's
/// multimodal content) and pixel dimensions for the "attached" chip. Pure data — the clipboard read and
/// PNG encoding happen in `tui::infrastructure::clipboard`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub data_url: String,
    pub width: usize,
    pub height: usize,
}

/// The multi-line input editor: a `tui-textarea` `TextArea` behind a thin domain wrapper. The wrapper
/// confines the widget type to this module and exposes only what the reducer and the renderer need —
/// full editor behaviour (selection, word motion, undo/redo, soft word-wrap) comes from the widget,
/// while clipboard side effects stay outside (the reducer is pure; the runtime performs them).
#[derive(Debug, Clone)]
pub struct InputBuffer {
    area: TextArea<'static>,
}

impl Default for InputBuffer {
    fn default() -> Self {
        let mut area = TextArea::default();
        area.set_wrap_mode(WrapMode::WordOrGlyph);
        area.remove_line_number();
        Self { area }
    }
}

impl InputBuffer {
    /// The whole buffer as a single string, logical lines joined by `\n`.
    pub fn text(&self) -> String {
        self.area.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.area.is_empty()
    }

    /// The cursor's logical `(row, col)` position in the buffer.
    pub fn cursor(&self) -> (usize, usize) {
        self.area.cursor()
    }

    /// The cursor's logical row — used to decide whether Up/Down should recall history (at the
    /// first/last row) or move the cursor within a multi-line buffer.
    pub fn cursor_row(&self) -> usize {
        self.cursor().0
    }

    pub fn last_row(&self) -> usize {
        self.area.lines().len().saturating_sub(1)
    }

    /// Move the cursor to a logical `(row, col)` position — e.g. a mouse click the renderer resolved
    /// to text coordinates. `tui-textarea` clamps out-of-range values to the buffer, so it never panics.
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.area
            .move_cursor(CursorMove::Jump(row as u16, col as u16));
    }

    /// Insert a string at the cursor (bracketed paste of text).
    pub fn insert(&mut self, s: &str) {
        self.area.insert_str(s);
    }

    pub fn newline(&mut self) {
        self.area.insert_newline();
    }

    /// Feed a widget input event (the reducer maps a key press to it), returning whether it mutated the
    /// text. This is the single path for ordinary editing: typing, deletion, cursor motion, selection.
    pub fn feed(&mut self, input: Input) -> bool {
        self.area.input(input)
    }

    pub fn is_selecting(&self) -> bool {
        self.area.is_selecting()
    }

    pub fn undo(&mut self) -> bool {
        self.area.undo()
    }

    pub fn redo(&mut self) -> bool {
        self.area.redo()
    }

    /// Copy the active selection into the OS clipboard text returned here (the caller performs the I/O).
    /// `None` when there is no selection.
    pub fn copy_selection(&mut self) -> Option<String> {
        if !self.area.is_selecting() {
            return None;
        }
        self.area.copy();
        let text = self.area.yank_text();
        (!text.is_empty()).then_some(text)
    }

    /// Cut the active selection: remove it from the buffer and return its text for the OS clipboard.
    pub fn cut_selection(&mut self) -> Option<String> {
        if !self.area.is_selecting() {
            return None;
        }
        self.area.cut();
        let text = self.area.yank_text();
        (!text.is_empty()).then_some(text)
    }

    /// Replace the whole buffer (history recall), placing the cursor at the end. This is a hard
    /// replacement: the widget's undo/redo stack is reset, so Ctrl+Z does not cross a recall (the
    /// pre-recall draft is recoverable through history navigation, not undo).
    pub fn set(&mut self, text: String) {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let row = lines.len().saturating_sub(1);
        let col = lines.last().map(|l| l.chars().count()).unwrap_or(0);
        self.area.set_lines(lines, (row, col));
    }

    /// Take the text out, leaving the buffer empty.
    pub fn take(&mut self) -> String {
        let text = self.text();
        self.area.set_lines(vec![String::new()], (0, 0));
        text
    }

    /// Apply the theme styles (base/cursor/selection) — set once by the runtime, which owns the theme.
    pub fn set_styles(&mut self, base: Style, cursor: Style, selection: Style) {
        self.area.set_style(base);
        self.area.set_cursor_line_style(base);
        self.area.set_cursor_style(cursor);
        self.area.set_selection_style(selection);
    }

    /// The widget for rendering. `&TextArea` implements `Widget`; the editor renders it directly.
    pub fn widget(&self) -> &TextArea<'static> {
        &self.area
    }
}

/// How a screen selection grows from a click: a plain drag selects by character; a double/triple click
/// selects the word/line under the cursor. The actual character ranges for `Word`/`Line` are derived
/// from the rendered buffer (only the overlay/runtime can see the glyphs), so the reducer only tags the
/// intent here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Granularity {
    #[default]
    Char,
    Word,
    Line,
}

/// What the selection is waiting for the runtime to do on the next draw. The copy must happen in the
/// runtime (it scrapes the rendered buffer), so the reducer can only request it: `CopyAndKeep` (mouse
/// release — leave the highlight up) or `CopyAndClear` (Ctrl+C — drop it after, so the next Ctrl+C is
/// free to cancel/quit again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionState {
    #[default]
    Idle,
    CopyAndKeep,
    CopyAndClear,
}

/// A text selection over the rendered screen, in absolute terminal cells. It lives in screen space (not
/// source text), so it works uniformly over the transcript, tool output, and the composer. The reducer
/// sets `anchor`/`head`/`granularity`/`state`; the overlay paints it and the runtime scrapes the cells
/// to copy. `Copy` so the runtime can lift it out of the model without holding a borrow across a draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenSelection {
    /// Where the gesture began (fixed end).
    pub anchor: (u16, u16),
    /// The moving end (follows the drag / last click).
    pub head: (u16, u16),
    pub granularity: Granularity,
    pub state: SelectionState,
}

impl ScreenSelection {
    pub fn new(col: u16, row: u16, granularity: Granularity) -> Self {
        Self {
            anchor: (col, row),
            head: (col, row),
            granularity,
            state: SelectionState::Idle,
        }
    }

    /// Move the head; the anchor stays put.
    pub fn extend(&mut self, col: u16, row: u16) {
        self.head = (col, row);
    }

    /// A character selection collapses to nothing when anchor == head (a bare click). A word/line
    /// selection is never empty — even a single click expands to the word/line under it.
    pub fn is_empty(&self) -> bool {
        self.granularity == Granularity::Char && self.anchor == self.head
    }

    /// `(start, end)` ordered by row then column, so the overlay never special-cases drag direction.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let key = |(c, r): (u16, u16)| (r, c);
        if key(self.anchor) <= key(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// Submitted-prompt history with shell-style up/down recall. The in-progress line is saved as a draft
/// when navigation starts and restored when navigating past the newest entry.
#[derive(Debug, Default, Clone)]
pub struct History {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: String,
}

impl History {
    /// Record a submitted line (trimmed, non-empty, de-duplicated against the last) and reset navigation.
    pub fn record(&mut self, line: &str) {
        self.cursor = None;
        self.draft.clear();
        let line = line.trim();
        if line.is_empty() || self.entries.last().is_some_and(|last| last == line) {
            return;
        }
        self.entries.push(line.to_string());
    }

    /// Step to an older entry, saving `current` as the draft on the first step.
    pub fn older(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        match self.cursor {
            None => {
                self.draft = current.to_string();
                self.cursor = Some(self.entries.len() - 1);
            }
            Some(0) => {}
            Some(i) => self.cursor = Some(i - 1),
        }
        self.cursor.map(|i| self.entries[i].clone())
    }

    /// Step to a newer entry; past the newest, return the saved draft.
    pub fn newer(&mut self) -> Option<String> {
        match self.cursor {
            None => None,
            Some(i) if i + 1 < self.entries.len() => {
                self.cursor = Some(i + 1);
                Some(self.entries[i + 1].clone())
            }
            Some(_) => {
                self.cursor = None;
                Some(std::mem::take(&mut self.draft))
            }
        }
    }
}

/// Transcript scroll position, measured as lines scrolled up from the newest content. Zero means
/// pinned to the bottom (auto-following new output). The view clamps it to the available scrollback.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    pub scrollback: u16,
}

impl Scroll {
    pub fn up(&mut self, lines: u16) {
        self.scrollback = self.scrollback.saturating_add(lines);
    }

    pub fn down(&mut self, lines: u16) {
        self.scrollback = self.scrollback.saturating_sub(lines);
    }

    pub fn pin(&mut self) {
        self.scrollback = 0;
    }

    /// Jump to the top of the scrollback. The view clamps to the available history, so saturating to
    /// the maximum is enough — no viewport height needs to leak into the model.
    pub fn top(&mut self) {
        self.scrollback = u16::MAX;
    }
}

/// The options shown for a tool-call confirmation, in display order. `PendingApproval.selected` indexes
/// this list; the keymap maps the chosen index to an approval decision (option 1 also switches to auto).
pub const APPROVAL_OPTIONS: [&str; 3] = ["Sim", "Sim, e não perguntar de novo (modo auto)", "Não"];

/// A tool-call (or runaway-checkpoint) confirmation awaiting the user's answer. Pure data — the reply
/// channel lives in the runtime, since the engine handles approvals one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub prompt: String,
    pub default_accept: bool,
    /// The highlighted option index into `APPROVAL_OPTIONS`.
    pub selected: usize,
}

impl PendingApproval {
    /// A new pending approval, highlighting the option matching the default (accept → "Sim",
    /// decline → "Não").
    pub fn new(prompt: String, default_accept: bool) -> Self {
        let selected = if default_accept {
            0
        } else {
            APPROVAL_OPTIONS.len() - 1
        };
        Self {
            prompt,
            default_accept,
            selected,
        }
    }

    /// The confirmation question without the trailing `[S/n]`/`[s/N]` hint — the rich box shows the
    /// selectable options instead of the inline default.
    pub fn action(&self) -> &str {
        self.prompt
            .trim_end()
            .trim_end_matches("[S/n]")
            .trim_end_matches("[s/N]")
            .trim_end()
    }
}

/// The options shown when a plan-mode turn finishes: run the plan (confirming each step or fully
/// unattended in auto), keep refining it, or leave plan mode.
pub const PLAN_OPTIONS: [&str; 4] = [
    "Executar o plano",
    "Executar o plano em modo auto",
    "Continuar planejando",
    "Cancelar (sair do modo plan)",
];

/// A finished plan-mode turn awaiting the user's decision. The plan itself is the assistant's last
/// transcript item; this only tracks which action is highlighted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingPlan {
    /// The highlighted option index into `PLAN_OPTIONS`.
    pub selected: usize,
}

/// Which setting a generic picker chooses, so the keymap maps the highlighted row to the right effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Models,
    Effort,
    Provider,
    Sessions,
}

/// The last row of the `/provider` picker — selecting it opens the add-provider wizard instead of
/// switching. A sentinel label, never a real provider id.
pub const ADD_PROVIDER_LABEL: &str = "+ adicionar novo provider";

/// The provider kinds the wizard offers, in display order. NVIDIA leads (the seeded default), so it is
/// preselected at first-run onboarding; the rest follow. Vendor kinds require an API key; the generic
/// OpenAI-compatible and custom kinds may be keyless (Ollama / LM Studio) — the typed key decides.
pub const WIZARD_KINDS: [ProviderKind; 5] = [
    ProviderKind::Nvidia,
    ProviderKind::Anthropic,
    ProviderKind::Openai,
    ProviderKind::OpenAiCompatible,
    ProviderKind::Custom,
];

/// The steps of the add-provider wizard, in order. `ProviderId` is shown only for the keyless-capable
/// kinds (OpenAI-compatible / custom), so the user can name several coexisting endpoints; vendor kinds
/// use a canonical id and skip it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Kind,
    ProviderId,
    BaseUrl,
    Model,
    ExtraModels,
    ApiKey,
}

/// The add-provider wizard's accumulated state. Each text step edits its own field directly; the `Kind`
/// step moves `kind_selected`. The API key is redacted in `Debug` so it can never land in a log even
/// though `Model` derives `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderWizard {
    pub step: WizardStep,
    pub kind_selected: usize,
    /// The user-chosen provider id (keyless-capable kinds only; vendor kinds use a canonical token).
    pub id: String,
    pub base_url: String,
    pub model: String,
    pub extra_models: String,
    pub api_key: String,
    /// True when the wizard is the first-run onboarding flow (welcome framing; cancelling keeps the
    /// submit gate up instead of stranding a credential-less app).
    pub onboarding: bool,
}

impl std::fmt::Debug for ProviderWizard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderWizard")
            .field("step", &self.step)
            .field("kind", &self.kind())
            .field("id", &self.id)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("extra_models", &self.extra_models)
            .field("api_key", &"***")
            .field("onboarding", &self.onboarding)
            .finish()
    }
}

impl ProviderWizard {
    pub fn new() -> Self {
        Self {
            step: WizardStep::Kind,
            kind_selected: 0,
            id: String::new(),
            base_url: String::new(),
            model: String::new(),
            extra_models: String::new(),
            api_key: String::new(),
            onboarding: false,
        }
    }

    /// The wizard opened at first run with no credential: the welcome framing, NVIDIA preselected
    /// (`kind_selected = 0`, the leading entry in [`WIZARD_KINDS`]). Built by mutating `new()` rather than
    /// struct-update syntax, which cannot move fields out of a `Drop` type.
    pub fn onboarding() -> Self {
        let mut wizard = Self::new();
        wizard.onboarding = true;
        wizard
    }

    /// The selected provider kind.
    pub fn kind(&self) -> ProviderKind {
        WIZARD_KINDS[self.kind_selected.min(WIZARD_KINDS.len() - 1)]
    }

    /// Whether the selected kind requires an API key (vendor) or may be keyless (compatible / custom).
    pub fn key_required(&self) -> bool {
        self.kind().requires_api_key()
    }

    /// The kind's canonical id token (the wizard id for a vendor kind, and the fallback for a blank
    /// keyless id).
    fn canonical_id(&self) -> &'static str {
        match self.kind() {
            ProviderKind::Nvidia => "nvidia",
            ProviderKind::Openai => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::Custom => "custom",
        }
    }

    /// The id a finished wizard gives its provider. Vendor kinds use the canonical token (re-adding one
    /// reconfigures it). Keyless-capable kinds use the user-typed `id`, sanitized to a stable `[a-z0-9_-]`
    /// token, so several compatible endpoints (e.g. a local LM Studio and a remote OpenRouter) can coexist;
    /// a blank id falls back to the canonical token.
    pub fn provider_id(&self) -> String {
        if self.key_required() {
            return self.canonical_id().to_string();
        }
        let sanitized: String = self
            .id
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let sanitized = sanitized.trim_matches('-').to_string();
        if sanitized.is_empty() {
            self.canonical_id().to_string()
        } else {
            sanitized
        }
    }

    /// The model catalog: the default model first, then the comma-separated extras (trimmed, de-duped,
    /// blanks dropped).
    pub fn models(&self) -> Vec<String> {
        let mut models = Vec::new();
        let model = self.model.trim();
        if !model.is_empty() {
            models.push(model.to_string());
        }
        for extra in self.extra_models.split(',') {
            let extra = extra.trim();
            if !extra.is_empty() && !models.iter().any(|m| m == extra) {
                models.push(extra.to_string());
            }
        }
        models
    }

    /// The field the current step edits, or `None` on the `Kind` step.
    fn field_mut(&mut self) -> Option<&mut String> {
        match self.step {
            WizardStep::Kind => None,
            WizardStep::ProviderId => Some(&mut self.id),
            WizardStep::BaseUrl => Some(&mut self.base_url),
            WizardStep::Model => Some(&mut self.model),
            WizardStep::ExtraModels => Some(&mut self.extra_models),
            WizardStep::ApiKey => Some(&mut self.api_key),
        }
    }

    pub fn push_char(&mut self, c: char) {
        if let Some(field) = self.field_mut() {
            field.push(c);
        }
    }

    /// Append pasted text to the current field, dropping control characters (a pasted key often carries
    /// a trailing newline). A no-op on the `Kind` step. This is how an API key is pasted into the masked
    /// field instead of leaking into the plaintext composer.
    pub fn push_str(&mut self, text: &str) {
        if let Some(field) = self.field_mut() {
            field.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn backspace(&mut self) {
        if let Some(field) = self.field_mut() {
            field.pop();
        }
    }

    /// Move the kind highlight (only meaningful on the `Kind` step), wrapping.
    pub fn move_kind(&mut self, delta: i32) {
        if self.step != WizardStep::Kind {
            return;
        }
        let len = WIZARD_KINDS.len() as i32;
        self.kind_selected = (self.kind_selected as i32 + delta).rem_euclid(len) as usize;
    }
}

impl Default for ProviderWizard {
    fn default() -> Self {
        Self::new()
    }
}

/// Zeroize the API-key buffer when the wizard is dropped (cancel, reopen, or after finalize), so a key
/// the user typed but never submitted does not linger in freed memory. Matches the project's `Secret`/
/// `Zeroizing` discipline. (Reallocations during editing can still leave residue — inherent to a growable
/// buffer; the staged `Secret` zeroizes the submitted value.) `Drop` is compatible with the finalize
/// path, which extracts the key via `mem::take` rather than moving the field out.
impl Drop for ProviderWizard {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}

/// A generic single-choice picker modal (used by `/models` and `/effort`), rendered with the same
/// borderless stanza as the approval/plan boxes. `options` are the selectable labels in display order;
/// `selected` indexes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub kind: PickerKind,
    pub label: String,
    pub action: String,
    pub options: Vec<String>,
    pub selected: usize,
}

impl Picker {
    /// Open a picker, clamping `selected` into range (or 0 when there are no options).
    pub fn new(
        kind: PickerKind,
        label: impl Into<String>,
        action: impl Into<String>,
        options: Vec<String>,
        selected: usize,
    ) -> Self {
        let selected = selected.min(options.len().saturating_sub(1));
        Self {
            kind,
            label: label.into(),
            action: action.into(),
            options,
            selected,
        }
    }

    /// Move the highlight by `delta` rows, wrapping within the options.
    pub fn move_cursor(&mut self, delta: i32) {
        if self.options.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.options.len() as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(len) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_debug_redacts_the_api_key() {
        let mut w = ProviderWizard::new();
        w.step = WizardStep::ApiKey;
        w.api_key = "sk-super-secret".to_string();
        let rendered = format!("{w:?}");
        assert!(
            !rendered.contains("sk-super-secret"),
            "the API key leaked in Debug: {rendered}"
        );
        assert!(rendered.contains("***"));
    }

    #[test]
    fn wizard_models_puts_the_default_first_and_dedupes_extras() {
        let mut w = ProviderWizard::new();
        w.model = "  m1 ".to_string();
        w.extra_models = "m2, m1 , ,m3".to_string();
        assert_eq!(w.models(), vec!["m1", "m2", "m3"]);
    }

    #[test]
    fn wizard_provider_id_is_the_kind_token() {
        let w = ProviderWizard::new(); // kind index 0 = NVIDIA (the leading, preselected entry)
        assert_eq!(w.provider_id(), "nvidia");
        assert_eq!(w.kind(), ProviderKind::Nvidia);
    }

    #[test]
    fn key_required_only_for_vendor_kinds() {
        let mut w = ProviderWizard::new();
        // WIZARD_KINDS: [Nvidia, Anthropic, Openai, OpenAiCompatible, Custom].
        for (idx, required) in [(0, true), (1, true), (2, true), (3, false), (4, false)] {
            w.kind_selected = idx;
            assert_eq!(w.key_required(), required, "kind {:?}", w.kind());
        }
    }

    #[test]
    fn provider_id_sanitizes_a_named_keyless_provider() {
        let mut w = ProviderWizard::new();
        w.kind_selected = 3; // OpenAiCompatible — keyless-capable, so the typed id is used
        w.id = "  My LM Studio! ".to_string();
        assert_eq!(w.provider_id(), "my-lm-studio");
        // A blank id falls back to the canonical kind token, never an empty id.
        w.id = "   ".to_string();
        assert_eq!(w.provider_id(), "openai-compatible");
        // A vendor kind ignores the id field entirely.
        w.kind_selected = 0;
        w.id = "ignored".to_string();
        assert_eq!(w.provider_id(), "nvidia");
    }

    #[test]
    fn wizard_kinds_lead_with_nvidia() {
        assert_eq!(WIZARD_KINDS.len(), 5);
        assert_eq!(WIZARD_KINDS[0], ProviderKind::Nvidia);
    }

    #[test]
    fn onboarding_constructor_sets_flag_and_preselects_nvidia() {
        let w = ProviderWizard::onboarding();
        assert!(w.onboarding);
        assert_eq!(w.kind_selected, 0);
        assert_eq!(w.kind(), ProviderKind::Nvidia);
        assert_eq!(w.step, WizardStep::Kind);
    }

    #[test]
    fn insert_set_and_take_round_trip_the_text() {
        let mut b = InputBuffer::default();
        b.insert("ação");
        assert_eq!(b.text(), "ação");
        assert!(!b.is_empty());
        let taken = b.take();
        assert_eq!(taken, "ação");
        assert!(b.is_empty());
        b.set("ab\ncd".to_string());
        assert_eq!(b.text(), "ab\ncd");
        assert_eq!(b.last_row(), 1);
        assert_eq!(b.cursor_row(), 1); // set places the cursor at the end (second line)
    }

    #[test]
    fn set_cursor_jumps_to_a_logical_position() {
        let mut b = InputBuffer::default();
        b.set("abc\nde".to_string());
        b.set_cursor(1, 1);
        assert_eq!(b.cursor(), (1, 1));
    }

    #[test]
    fn set_cursor_clamps_beyond_the_buffer() {
        let mut b = InputBuffer::default();
        b.set("ab".to_string());
        b.set_cursor(9, 9); // both out of range — clamped to the only row and its end
        assert_eq!(b.cursor(), (0, 2));
    }

    #[test]
    fn copy_selection_is_none_without_a_selection() {
        let mut b = InputBuffer::default();
        b.insert("hello");
        assert!(!b.is_selecting());
        assert_eq!(b.copy_selection(), None);
    }

    #[test]
    fn screen_selection_is_empty_only_for_a_char_click() {
        // A bare char click (anchor == head) selects nothing; a word/line click or any drag does not.
        assert!(ScreenSelection::new(3, 2, Granularity::Char).is_empty());
        assert!(!ScreenSelection::new(3, 2, Granularity::Word).is_empty());
        assert!(!ScreenSelection::new(3, 2, Granularity::Line).is_empty());
        let mut s = ScreenSelection::new(3, 2, Granularity::Char);
        s.extend(4, 2);
        assert!(!s.is_empty());
    }

    #[test]
    fn history_recalls_older_then_restores_draft() {
        let mut h = History::default();
        h.record("first");
        h.record("second");
        assert_eq!(h.older("draft").as_deref(), Some("second"));
        assert_eq!(h.older("draft").as_deref(), Some("first"));
        assert_eq!(h.newer().as_deref(), Some("second"));
        assert_eq!(h.newer().as_deref(), Some("draft"));
    }

    #[test]
    fn history_skips_consecutive_duplicates() {
        let mut h = History::default();
        h.record("x");
        h.record("x");
        assert_eq!(h.older("").as_deref(), Some("x"));
        assert_eq!(h.older("").as_deref(), Some("x"));
    }

    #[test]
    fn scroll_top_saturates_and_pin_resets() {
        let mut s = Scroll::default();
        s.top();
        assert_eq!(s.scrollback, u16::MAX);
        s.pin();
        assert_eq!(s.scrollback, 0);
    }
}
