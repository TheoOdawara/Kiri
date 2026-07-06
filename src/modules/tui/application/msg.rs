use std::time::Duration;

use crate::modules::tui::domain::input_buffer::ImageAttachment;
use crate::modules::tui::domain::modal::PendingApproval;
use crate::modules::tui::domain::transcript::{ToolDiff, ToolStatus};

// The normalized input primitives live in `domain` (so `InputBuffer` can map a key onto the widget
// without the reducer touching `tui_textarea`); re-exported here so `application::msg::{Key, KeyPress,
// MouseKind}` keeps resolving for the reducer, update loop, and runtime.
pub use crate::modules::tui::domain::input::{Key, KeyPress, MouseKind};

/// Which stream a delta belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Reasoning,
    Content,
}

/// Everything that can change the model: UI events (from crossterm), engine events (from the bridge
/// channel), and the per-frame tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Key(KeyPress),
    /// A left mouse-button gesture at a terminal cell — drives in-app screen text selection.
    Mouse {
        kind: MouseKind,
        col: u16,
        row: u16,
    },
    Paste(String),
    /// An image read from the OS clipboard, staged as an attachment for the next prompt.
    ImageAttached(ImageAttachment),
    Resize,
    Tick,
    TurnBegan,
    StreamDelta(StreamKind, String),
    TurnFinished,
    /// A tool call started running (engine → TUI), with its display command, an optional edit diff, and
    /// whether it was `run_command` (the only tool whose output may carry the stderr-split marker).
    ToolStarted {
        command: String,
        diff: Option<ToolDiff>,
        is_run_command: bool,
    },
    /// A tool call finished (engine → TUI): its outcome status, full (capped) output, and duration.
    ToolFinished {
        status: ToolStatus,
        output: String,
        elapsed: Duration,
    },
    ApprovalRequested(PendingApproval),
    /// The user scrolled the mouse wheel up by one notch.
    ScrollUp,
    /// The user scrolled the mouse wheel down by one notch.
    ScrollDown,
    /// The agent-loop future resolved; reset per-turn UI state.
    TurnEnded,
}
