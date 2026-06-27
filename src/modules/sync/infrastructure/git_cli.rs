use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::modules::sync::application::git::{Git, GitOutput};
use crate::shared::kernel::error::AgentError;

/// Upper bound for a single git invocation. A push/pull reaching a remote can be slow, but must never
/// hang the CLI forever; the timeout kills the child (kill-on-drop) and surfaces a clear error.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// `Git` adapter that shells out to the system `git`. The user's existing credential helper / SSH agent
/// authenticates to the remote, so Kiri never handles repo credentials itself.
pub struct GitCli;

#[async_trait::async_trait]
impl Git for GitCli {
    async fn run(&self, args: &[&str], cwd: &Path) -> Result<GitOutput, AgentError> {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = command.spawn().map_err(|error| {
            AgentError::Sync(format!(
                "could not run git (is it installed and on PATH?): {error}"
            ))
        })?;

        let output = match tokio::time::timeout(GIT_TIMEOUT, child.wait_with_output()).await {
            Ok(result) => {
                result.map_err(|error| AgentError::Sync(format!("git failed to run: {error}")))?
            }
            Err(_) => {
                return Err(AgentError::Sync(format!(
                    "git timed out after {}s",
                    GIT_TIMEOUT.as_secs()
                )));
            }
        };

        Ok(GitOutput {
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            success: output.status.success(),
        })
    }
}
