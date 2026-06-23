use crate::modules::tui::domain::view_state::PendingApproval;

/// Which stream a delta belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Reasoning,
    Content,
}

/// A normalized key press, decoupled from crossterm so the reducer and key map stay library-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub code: Key,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
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

/// Everything that can change the model: UI events (from crossterm), engine events (from the bridge
/// channel), and the per-frame tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Key(KeyPress),
    Paste(String),
    Resize,
    Tick,
    TurnBegan,
    StreamDelta(StreamKind, String),
    TurnFinished,
    ApprovalRequested(PendingApproval),
    /// The user scrolled the mouse wheel up by one notch.
    ScrollUp,
    /// The user scrolled the mouse wheel down by one notch.
    ScrollDown,
    /// The agent-loop future resolved; reset per-turn UI state.
    TurnEnded,
}
