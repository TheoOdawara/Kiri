/// How tool calls are gated for a turn. The user cycles modes with Shift+Tab and the active mode is read
/// at the start of each turn. `Default` confirms every call; `Auto` runs them without asking; `Plan`
/// withholds destructive tools so the agent can only read and propose a plan to approve.
///
/// Lives in the kernel because it is a cross-cutting primitive shared by the agent, tools, and tui
/// contexts — keeping it here lets the tui domain reference it without depending on another module's
/// application layer.
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
