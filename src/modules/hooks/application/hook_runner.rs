use crate::modules::extensions::domain::resource::Hook;
use crate::modules::tools::application::sandbox::Sandbox;

/// The outcome of running one hook, for the caller to surface as a transcript notice. Never an error the
/// caller must propagate — hooks are fire-and-forget (ADR 0021): a failing or slow hook is reported, not
/// raised, so it can never fail the session/turn it fired from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookOutcome {
    pub hook_id: String,
    pub ok: bool,
    /// A short, single-line summary: the first line of output, or the failure/timeout reason.
    pub summary: String,
}

/// Port: execute one hook's shell command. Implemented once (`ShellHookRunner`, the sanctioned site for
/// this context's process I/O) over the sandbox's existing confined-exec surface — hooks never bypass
/// the harness's process confinement, the same guarantee `run_command` gets. `?Send`: `&dyn Sandbox` (like
/// `Tool::execute`) is not `Sync`, so the future held across the `.await` cannot be `Send`.
#[async_trait::async_trait(?Send)]
pub trait HookRunner {
    async fn run(&self, sandbox: &dyn Sandbox, hook: &Hook) -> HookOutcome;
}
