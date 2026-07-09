use crate::modules::tools::application::tool::Confirmation;

/// Why the checkpoint fired, so the prompt names the real cause rather than always reporting elapsed
/// minutes — which reads as "~0min" when the tool-call count is what tripped it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointReason {
    /// The wall-clock budget elapsed; carries the minutes the turn has run.
    Elapsed { minutes: u64 },
    /// The tool-call count since the last check-in reached the cap; carries that count.
    CallCount { calls: usize },
}

/// The user's decision on a tool call (or the runaway checkpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approved,
    /// Approve, then run the rest of the turn unattended: the engine switches to auto mode.
    ApprovedAuto,
    Declined,
    /// The user's input stream ended (or could not be read): end the session.
    Aborted,
}

/// The engine's port for gating tool calls behind user approval.
#[async_trait::async_trait(?Send)]
pub trait ApprovalPolicy {
    async fn decide(&mut self, confirmation: &Confirmation) -> Approval;
    /// The runaway checkpoint (elapsed time or tool-call count): keep going?
    async fn confirm_continue(&mut self, reason: CheckpointReason) -> Approval;
}
