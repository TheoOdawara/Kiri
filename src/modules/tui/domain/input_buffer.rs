use ratatui::style::Style;
use tui_textarea::{CursorMove, Input, Key as TaKey, TextArea, WrapMode};

use crate::modules::tui::domain::input::{Key, KeyPress};

/// A pasted image staged for the next prompt: its data URL (base64 PNG, ready for the provider's
/// multimodal content) and pixel dimensions for the "attached" chip. Pure data — the clipboard read and
/// PNG encoding happen in `tui::infrastructure::runtime::clipboard`.
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
        // Visible caret out of the box so field editors (wizard/picker/search) that never go through
        // the runtime's themed `set_styles` still show where the user is typing. The composer overrides
        // this with brand colours at boot.
        area.set_cursor_style(Style::default().add_modifier(ratatui::style::Modifier::REVERSED));
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

    /// Feed a normalized key press, returning whether it mutated the text. The single path for ordinary
    /// editing (typing, deletion, cursor motion, selection): the reducer passes a backend-agnostic
    /// `KeyPress` and the widget-specific translation stays inside this sanctioned widget owner (ADR 0017),
    /// so the keymap never touches `tui_textarea`.
    pub fn feed_key(&mut self, key: KeyPress) -> bool {
        self.area.input(to_input(key))
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

/// Map a normalized key press onto the editor widget's backend-agnostic input type.
///
/// macOS word ops are translated onto the bindings the widget already understands: terminals send
/// Option as Alt, but the widget binds word motion to `Ctrl+Left/Right` (and meta `Alt+b/f`) and
/// word-delete to `Alt+Backspace/Delete`. So `Option+←/→` is rewritten to `Ctrl+←/→` — with `alt: false`,
/// which is mandatory: the widget has a `Ctrl+Alt+Left -> line head` arm that would otherwise win.
/// `Ctrl+Backspace/Delete` (Windows/Linux muscle memory) is rewritten to the widget's `Alt` word-delete.
fn to_input(key: KeyPress) -> Input {
    match (key.ctrl, key.alt, key.code) {
        (false, true, Key::Left) => {
            return Input {
                key: TaKey::Left,
                ctrl: true,
                alt: false,
                shift: key.shift,
            };
        }
        (false, true, Key::Right) => {
            return Input {
                key: TaKey::Right,
                ctrl: true,
                alt: false,
                shift: key.shift,
            };
        }
        (true, false, Key::Backspace) => {
            return Input {
                key: TaKey::Backspace,
                ctrl: false,
                alt: true,
                shift: false,
            };
        }
        (true, false, Key::Delete) => {
            return Input {
                key: TaKey::Delete,
                ctrl: false,
                alt: true,
                shift: false,
            };
        }
        _ => {}
    }
    Input {
        key: match key.code {
            Key::Char(c) => TaKey::Char(c),
            Key::Enter => TaKey::Enter,
            Key::Backspace => TaKey::Backspace,
            Key::Delete => TaKey::Delete,
            Key::Left => TaKey::Left,
            Key::Right => TaKey::Right,
            Key::Up => TaKey::Up,
            Key::Down => TaKey::Down,
            Key::Home => TaKey::Home,
            Key::End => TaKey::End,
            Key::PageUp => TaKey::PageUp,
            Key::PageDown => TaKey::PageDown,
            Key::Esc => TaKey::Esc,
            Key::Tab => TaKey::Tab,
            Key::BackTab => TaKey::Null,
        },
        ctrl: key.ctrl,
        alt: key.alt,
        shift: key.shift,
    }
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
}
