use crate::shared::kernel::error::AgentError;

/// The engine's output port to the user interface. Rendering of streamed deltas is the provider's
/// `EventSink`; this covers the rest, so the agent loop never writes to stdout/stderr directly.
pub trait Presenter {
    /// Reset any per-turn rendering state before a new turn starts streaming.
    fn begin_turn(&mut self);
    /// Called once a turn's stream ends: erase a leftover spinner, reset the terminal, newline.
    fn finish_turn(&mut self) -> Result<(), AgentError>;
    /// An out-of-band line (the workspace path, an error message).
    fn notice(&mut self, line: &str);
}
