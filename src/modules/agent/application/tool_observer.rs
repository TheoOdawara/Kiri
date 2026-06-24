use std::time::Duration;

use crate::modules::tools::application::tool::ToolOutcome;
use crate::shared::kernel::tool_call::ToolCall;

/// The engine's output port for observing tool activity, fired around every execution in **all**
/// approval modes — so the UI can show each command, result, and edit even when the user is not
/// prompted (auto/plan). Separate from `Presenter` (per-turn lifecycle) because this is per-call:
/// it fires N times within a turn. Synchronous, like `Presenter::begin_turn` — the production
/// adapter just pushes a message onto a channel and never blocks.
pub trait ToolObserver {
    /// A tool call is about to run (or be refused). `command` is the bare command label the registry
    /// derived from the call, for display (e.g. `edit src/x.rs`, `cat foo`, `rg 'q' .`).
    fn tool_started(&mut self, call: &ToolCall, command: &str);
    /// The tool call finished — executed, errored, declined, or blocked. `elapsed` measures only the
    /// execution, not any time spent awaiting the user's approval.
    fn tool_finished(&mut self, call: &ToolCall, outcome: &ToolOutcome, elapsed: Duration);
}
