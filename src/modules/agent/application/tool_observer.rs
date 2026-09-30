use std::time::Duration;

use crate::modules::tools::application::tool::ToolOutcome;
use crate::shared::kernel::tool_call::ToolCall;

/// Fires around every execution in **all** approval modes, so the UI shows each command even when the
/// user is not prompted. Per-call, unlike `Presenter`'s per-round lifecycle. Synchronous: the production
/// adapter only pushes onto a channel and never blocks.
pub trait ToolObserver {
    /// A tool call is about to run, or be refused. `command` is a display label, e.g. `edit src/x.rs`.
    fn tool_started(&mut self, call: &ToolCall, command: &str);
    /// Executed, errored, declined, or blocked. `elapsed` excludes time awaiting the user's approval.
    fn tool_finished(&mut self, call: &ToolCall, outcome: &ToolOutcome, elapsed: Duration);
}
