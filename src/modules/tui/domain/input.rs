//! Normalized terminal input primitives, decoupled from crossterm and from the editor widget. Their home
//! is `domain` (not `application/msg`) so the sanctioned widget owner `InputBuffer` can map a `KeyPress`
//! onto the widget's input type without the reducer ever touching `tui_textarea` (closes the keymap leak).

/// A normalized key press, decoupled from crossterm so the reducer and key map stay library-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub code: Key,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Which phase of a left-button mouse gesture a `Msg::Mouse` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Drag,
    Up,
}

/// The subset of keys the TUI acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Esc,
    Tab,
    /// Shift+Tab (crossterm reports it as a distinct back-tab key code).
    BackTab,
}
