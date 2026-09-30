use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::modules::sync::application::git::{Git, GitOutput};
use crate::modules::tools::infrastructure::exec::scrub_tokio_env;
use crate::shared::kernel::error::{AgentError, AgentResult};

/// Generous, because a push/pull reaching a remote can be slow — but never unbounded.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Shells out to the system `git`, so the user's own credential helper / SSH agent authenticates to
/// the remote and Kiri never handles repo credentials itself.
pub struct GitCli;

#[async_trait::async_trait]
impl Git for GitCli {
    async fn run(&self, args: &[&str], cwd: &Path) -> AgentResult<GitOutput> {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // #59: same env scrub as run_command/hooks — do not leak harness API keys into git/hooks.
        scrub_tokio_env(&mut command, |key| std::env::var(key).ok());

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
