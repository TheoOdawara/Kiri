use crate::modules::agent::application::approval_policy::Approval;

/// A side effect the pure reducer requests of the runtime, which owns the engine handles. The reducer
/// itself performs no I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Push the prompt as a user message and arm a new agent turn.
    SubmitPrompt(String),
    /// Answer the pending approval through its reply channel.
    AnswerApproval(Approval),
    /// Cooperatively cancel the running turn.
    CancelTurn,
    /// Tear down the TUI and end the session.
    Quit,
    /// Discard the conversation and start a fresh session.
    NewSession,
    /// Move the active workspace (sandbox root) to the given `/cd` path argument.
    ChangeWorkspace(String),
    /// Approve the proposed plan: leave plan mode and run a turn that executes it.
    ApprovePlan,
}
