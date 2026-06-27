use crate::shared::kernel::error::AgentError;

/// The engine's output port to the user interface. Rendering of streamed deltas is the provider's
/// `EventSink`; this covers the rest, so the agent loop never writes to stdout/stderr directly.
pub trait Presenter {
    /// Reset per-stream rendering state before a provider completion starts streaming. NOTE: this fires
    /// once per provider round, i.e. N times within one multi-round user turn (before each
    /// `provider.complete`), not once per user turn — name an adapter's state accordingly.
    fn begin_turn(&mut self);
    /// Called once a provider completion's stream ends (again, per round): erase a leftover spinner,
    /// reset the terminal, newline.
    fn finish_turn(&mut self) -> Result<(), AgentError>;
}
