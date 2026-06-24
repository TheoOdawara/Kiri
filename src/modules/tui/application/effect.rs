use crate::modules::agent::application::approval_policy::{Approval, ApprovalMode};

/// A side effect the pure reducer requests of the runtime, which owns the engine handles. The reducer
/// itself performs no I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Push the prompt (text plus any pasted image data URLs) as a user message and arm a new turn.
    SubmitPrompt { text: String, images: Vec<String> },
    /// Copy the given text to the OS clipboard.
    CopyToClipboard(String),
    /// Read the OS clipboard (image preferred, else text) and route it back into the buffer.
    PasteClipboard,
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
    /// Approve the proposed plan: leave plan mode and run a turn that executes it under the given
    /// mode (`Default` confirms each step, `Auto` runs the whole plan unattended).
    ApprovePlan(ApprovalMode),
}
