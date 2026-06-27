use crate::shared::kernel::error::AgentError;

/// The engine's output port to the user interface. Rendering of streamed deltas is the provider's
/// `EventSink`; this covers the rest, so the agent loop never writes to stdout/stderr directly.
pub trait Presenter {
    /// Reset per-stream rendering state before a provider completion starts streaming. Fires once per
    /// provider round — i.e. N times within one multi-round user turn, before each `provider.complete`.
    fn begin_round(&mut self);
    /// Called once a provider completion's stream ends (again, per round): erase a leftover spinner,
    /// reset the terminal, newline.
    fn finish_round(&mut self) -> Result<(), AgentError>;
}
