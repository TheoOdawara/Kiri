use crate::shared::kernel::error::AgentError;

/// The engine's output port to the UI, so the agent loop never writes to stdout/stderr directly. Streamed
/// deltas are the provider's `EventSink`; this covers the rest.
pub trait Presenter {
    /// Fires once per provider round, so N times within a multi-round user turn.
    fn begin_round(&mut self);
    /// Per round, once the stream ends: erase a leftover spinner, reset the terminal, newline.
    fn finish_round(&mut self) -> Result<(), AgentError>;
}
