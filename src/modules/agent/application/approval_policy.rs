use crate::modules::tools::application::tool::Confirmation;

/// The user's decision on a tool call (or the runaway checkpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approved,
    Declined,
    /// The user's input stream ended (or could not be read): end the session.
    Aborted,
}

/// The engine's port for gating tool calls behind user approval. The interactive implementation reads
/// a yes/no line from the terminal; a test implementation can auto-approve.
#[async_trait::async_trait(?Send)]
pub trait ApprovalPolicy {
    /// Present a tool confirmation and decide.
    async fn decide(&mut self, confirmation: &Confirmation) -> Approval;
    /// The wall-clock runaway checkpoint: after a long turn, keep going?
    async fn confirm_continue(&mut self, minutes: u64) -> Approval;
}
