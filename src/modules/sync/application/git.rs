use std::path::Path;

use async_trait::async_trait;

use crate::shared::kernel::error::AgentError;

/// The captured result of a git invocation.
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Port for the git operations the sync service needs. Implemented by `GitCli` (shells out to the
/// system `git`, so the user's own credential helper / SSH handles repo auth). A port so the service is
/// testable against a fake without a real repository or network.
#[async_trait]
pub trait Git: Send + Sync {
    /// Run `git <args>` in `cwd`, returning its captured output. A non-zero exit is reported via
    /// `GitOutput.success`, not as an `Err` — only a failure to launch/await the process is `Err`.
    async fn run(&self, args: &[&str], cwd: &Path) -> Result<GitOutput, AgentError>;
}
