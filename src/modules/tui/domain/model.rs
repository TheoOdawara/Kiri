use crate::modules::agent::application::approval_policy::ApprovalMode;

use super::command_menu::CommandMenu;
use super::transcript::Transcript;
use super::view_state::{
    History, ImageAttachment, InputBuffer, PendingApproval, PendingPlan, Scroll,
};

/// The status line's data: the model id, the active workspace, and the live turn indicators.
#[derive(Debug, Default)]
pub struct Status {
    pub model: String,
    pub workspace: String,
    pub streaming: bool,
    pub elapsed_secs: u64,
    pub spinner_frame: usize,
}

impl Status {
    /// Elapsed time as a compact label: seconds under a minute, `Mm Ss` once it reaches one. The raw
    /// seconds field stays the single source of truth; this is a render-only projection.
    pub fn elapsed_label(&self) -> String {
        if self.elapsed_secs < 60 {
            format!("{}s", self.elapsed_secs)
        } else {
            format!("{}m {}s", self.elapsed_secs / 60, self.elapsed_secs % 60)
        }
    }
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
    /// The live slash-command preview, open while the input starts with `/` and has no whitespace yet.
    pub command_menu: Option<CommandMenu>,
    /// Images pasted from the clipboard, staged for the next prompt and drained on submit.
    pub attachments: Vec<ImageAttachment>,
    /// When set, tool outputs and edit diffs render in full instead of a bounded preview. Toggled
    /// with Ctrl+O.
    pub expand_tools: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_label_formats_seconds_below_a_minute() {
        let s = Status {
            elapsed_secs: 0,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "0s");
        let s = Status {
            elapsed_secs: 59,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "59s");
    }

    #[test]
    fn elapsed_label_formats_minutes_and_seconds_at_and_above_a_minute() {
        let s = Status {
            elapsed_secs: 60,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "1m 0s");
        let s = Status {
            elapsed_secs: 125,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "2m 5s");
    }
}
