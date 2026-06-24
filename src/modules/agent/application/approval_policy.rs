use crate::modules::tools::application::tool::Confirmation;

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

/// How tool calls are gated for a turn. The user cycles modes with Shift+Tab and the active mode is read
/// at the start of each turn. `Default` confirms every call; `Auto` runs them without asking; `Plan`
/// withholds destructive tools so the agent can only read and propose a plan to approve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    #[default]
    Default,
    Auto,
    Plan,
}

impl ApprovalMode {
    /// The next mode in the Shift+Tab cycle: Default -> Auto -> Plan -> Default.
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Auto,
            Self::Auto => Self::Plan,
            Self::Plan => Self::Default,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_to_default_and_cycles() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Default);
        assert_eq!(ApprovalMode::Default.next(), ApprovalMode::Auto);
        assert_eq!(ApprovalMode::Auto.next(), ApprovalMode::Plan);
        assert_eq!(ApprovalMode::Plan.next(), ApprovalMode::Default);
    }
}
