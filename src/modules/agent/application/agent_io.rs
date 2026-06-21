use crate::modules::agent::application::approval_policy::ApprovalPolicy;
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::provider::application::completion_provider::EventSink;

/// The engine's single UI port: render streamed deltas (`EventSink`), finish a turn and emit notices
/// (`Presenter`), and prompt for a decision (`ApprovalPolicy`). One object — the terminal — owns all
/// console I/O and satisfies all three, so the agent loop borrows it once. The blanket impl makes any
/// type that implements the three an `AgentIo`.
pub trait AgentIo: EventSink + Presenter + ApprovalPolicy {}

impl<T: EventSink + Presenter + ApprovalPolicy> AgentIo for T {}
