/// How tool calls are gated for a turn, read once at the turn's start. It lives in the kernel so the tui
/// domain can reference it without depending on another module's application layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    /// Confirms every call.
    #[default]
    Default,
    /// Runs every call without asking.
    Auto,
    /// Withholds destructive tools; the agent can only read and propose a plan.
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
