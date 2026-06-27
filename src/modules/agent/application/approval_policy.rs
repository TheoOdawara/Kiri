use crate::modules::tools::application::tool::Confirmation;

/// Why the runaway checkpoint fired, so the prompt states the real cause instead of always phrasing it
/// as elapsed minutes (which read as "~0min" when it was actually the tool-call count that tripped it).
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
    /// Approve this call and run the rest of the turn unattended — the user chose "auto" at the
    /// prompt, so the engine switches to auto mode for the remaining calls instead of asking again.
    ApprovedAuto,
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
    /// The runaway checkpoint (elapsed time or tool-call count): keep going?
    async fn confirm_continue(&mut self, reason: CheckpointReason) -> Approval;
}
