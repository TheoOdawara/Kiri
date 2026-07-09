use crate::modules::extensions::domain::resource::Hook;
use crate::modules::tools::application::sandbox::Sandbox;

/// Fire-and-forget (ADR 0021): a failing or slow hook is reported as a transcript notice, never raised,
/// so it can never fail the session/turn it fired from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookOutcome {
    pub hook_id: String,
    pub ok: bool,
    /// A short, single-line summary: the first line of output, or the failure/timeout reason.
    pub summary: String,
}

/// Runs over the sandbox's confined-exec surface: a hook never bypasses the process confinement
/// `run_command` gets. `?Send`: `&dyn Sandbox` is not `Sync`, so the future across the `.await` cannot be.
#[async_trait::async_trait(?Send)]
pub trait HookRunner {
    async fn run(&self, sandbox: &dyn Sandbox, hook: &Hook) -> HookOutcome;
}
