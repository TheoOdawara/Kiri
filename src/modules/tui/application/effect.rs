use crate::modules::agent::application::approval_policy::Approval;
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::provider::{Effort, ProviderKind};

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
    /// Switch the active model (from the `/models` picker). The runtime applies it to the engine and
    /// persists it on the active provider in the global config.
    SetModel(String),
    /// Switch the reasoning effort (from the `/effort` picker). The runtime rebuilds the provider with
    /// the new effort, applies it, and persists it in the global config.
    SetEffort(Effort),
    /// Switch the active provider (from the `/provider` picker). The runtime rebuilds the adapter with
    /// the target's stored credential, adopts its model, and persists the active selection.
    SetProvider(String),
    /// Save a new provider from the add wizard and make it active. Carries only non-secret fields — the
    /// typed API key is staged separately in `Model::pending_credential` (taken by the runtime), so it
    /// never rides in a `Debug`-printable effect.
    SaveProvider {
        id: String,
        kind: ProviderKind,
        base_url: String,
        model: String,
        models: Vec<String>,
    },
    /// Place the edit cursor at the composer click (absolute screen cell). The runtime resolves it
    /// against the rendered editor geometry — a no-op when the click is outside the box or the layout
    /// is ambiguous (wrapped/scrolled), since the reducer has no render geometry to map it itself.
    PlaceCursor { col: u16, row: u16 },
}
