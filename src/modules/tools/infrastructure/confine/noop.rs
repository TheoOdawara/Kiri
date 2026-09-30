use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};
use crate::shared::kernel::error::AgentError;

/// The no-op confinement adapter: returns the command unchanged. Used on platforms without an OS
/// sandbox facility (Windows, BSD) and whenever `KIRI_SANDBOX=off`. Path validation and the
/// confirmation layer remain the active guards, so behavior equals the pre-sandbox tool surface.
#[derive(Debug)]
pub struct NoConfinement;

impl CommandSandbox for NoConfinement {
    fn confine(
        &self,
        cmd: tokio::process::Command,
        _policy: &SandboxPolicy,
    ) -> Result<tokio::process::Command, AgentError> {
        Ok(cmd)
    }

    fn supports_confinement(&self) -> bool {
        false
    }
}
