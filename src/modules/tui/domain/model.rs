use crate::modules::agent::application::approval_policy::ApprovalMode;

use super::transcript::Transcript;
use super::view_state::{History, InputBuffer, PendingApproval, PendingPlan, Scroll};

/// The status line's data: the model id, the active workspace, and the live turn indicators.
#[derive(Debug, Default)]
pub struct Status {
    pub model: String,
    pub workspace: String,
    pub streaming: bool,
    pub elapsed_secs: u64,
    pub spinner_frame: usize,
}

/// The whole TUI state — a pure value mutated only by `update`. The runtime renders it and feeds it
/// messages; it never holds engine handles (channels/conversation live in the runtime).
#[derive(Debug, Default)]
pub struct Model {
    pub transcript: Transcript,
    pub input: InputBuffer,
    pub history: History,
    pub scroll: Scroll,
    pub status: Status,
    /// A confirmation awaiting an answer; while set, keys answer it instead of editing.
    pub pending_approval: Option<PendingApproval>,
    /// A finished plan awaiting the user's decision; while set, keys drive the plan box.
    pub pending_plan: Option<PendingPlan>,
    /// A turn is running (the agent loop future is armed).
    pub busy: bool,
    pub should_quit: bool,
    /// How tool calls are gated; cycled with Shift+Tab, read at the start of each turn.
    pub approval_mode: ApprovalMode,
}

impl Model {
    pub fn new(model: String, workspace: String) -> Self {
        Self {
            status: Status {
                model,
                workspace,
                ..Status::default()
            },
            ..Self::default()
        }
    }
}
