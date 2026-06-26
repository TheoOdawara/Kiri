use ratatui::style::Style;
use tui_textarea::{Input, TextArea, WrapMode};

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

    /// The cursor's logical row and the index of the last row — used to decide whether Up/Down should
    /// recall history (at the first/last row) or move the cursor within a multi-line buffer.
    pub fn cursor_row(&self) -> usize {
        self.area.cursor().0
    }

    pub fn last_row(&self) -> usize {
        self.area.lines().len().saturating_sub(1)
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

#[cfg(test)]
mod tests {
    use super::*;

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
