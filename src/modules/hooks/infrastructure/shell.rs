//! The sanctioned site for the `hooks` context's process I/O (ADR 0021): runs a hook's shell command
//! through the harness's existing confined-exec surface (`tools::infrastructure::exec::run_shell` — the
//! same one `run_command` uses), so a hook can never bypass the sandbox's OS-level confinement or reach
//! the network by default.

use std::time::Duration;

use crate::modules::extensions::domain::resource::Hook;
use crate::modules::hooks::application::hook_runner::{HookOutcome, HookRunner};
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::infrastructure::exec::{self, ExecError, capped_combined};
use crate::shared::kernel::sandbox::NetworkPolicy;

/// Bound on how long a single hook may run before it is killed. Hooks are auxiliary and notice-only, so
/// there is never a reason to wait as long as a model-driven `run_command` call might.
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// Executes a hook's shell command confined to the workspace, network-denied by default (a hook is a
/// notification point, not an integration — a hook that genuinely needs network is exactly what MCP is
/// for, Fase 5).
pub struct ShellHookRunner;

#[async_trait::async_trait(?Send)]
impl HookRunner for ShellHookRunner {
    async fn run(&self, sandbox: &dyn Sandbox, hook: &Hook) -> HookOutcome {
        let policy = sandbox.command_policy(NetworkPolicy::Deny, &[], &[]);
        let result = exec::run_shell(
            &hook.command,
            Some(sandbox.root()),
            HOOK_TIMEOUT,
            sandbox.confiner(),
            &policy,
        )
        .await;
        match result {
            Ok(res) => {
                let ok = res.exit_code == Some(0);
                let output = capped_combined(&res);
                let summary = first_line(&output).unwrap_or_else(|| {
                    format!(
                        "exit {}",
                        res.exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "?".to_string())
                    )
                });
                HookOutcome {
                    hook_id: hook.id.clone(),
                    ok,
                    summary,
                }
            }
            Err(ExecError::Timeout(secs)) => HookOutcome {
                hook_id: hook.id.clone(),
                ok: false,
                summary: format!("timed out after {secs}s"),
            },
            Err(ExecError::Spawn(error)) => HookOutcome {
                hook_id: hook.id.clone(),
                ok: false,
                summary: format!("failed to start: {error}"),
            },
        }
    }
}

/// The first non-blank line of `text`, trimmed — a one-line summary for the transcript notice.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::extensions::domain::resource::HookEvent;
    use crate::modules::extensions::domain::scope::Layer;
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use tempfile::TempDir;

    fn hook(command: &str) -> Hook {
        Hook {
            id: "test-hook".to_string(),
            event: HookEvent::SessionStart,
            matcher: None,
            command: command.to_string(),
            layer: Layer::Global,
            path: "/fake/test-hook.md".to_string(),
        }
    }

    #[tokio::test]
    async fn a_successful_command_reports_ok_with_its_first_output_line() {
        let dir = TempDir::new().unwrap();
        let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ShellHookRunner
            .run(&sandbox, &hook("echo hello world"))
            .await;
        assert!(outcome.ok);
        assert_eq!(outcome.summary, "hello world");
        assert_eq!(outcome.hook_id, "test-hook");
    }

    #[tokio::test]
    async fn a_failing_command_reports_not_ok() {
        let dir = TempDir::new().unwrap();
        let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ShellHookRunner.run(&sandbox, &hook("exit 1")).await;
        assert!(!outcome.ok);
    }

    #[tokio::test]
    async fn a_command_with_no_output_summarizes_the_exit_code() {
        let dir = TempDir::new().unwrap();
        let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ShellHookRunner.run(&sandbox, &hook("true")).await;
        assert!(outcome.ok);
        assert_eq!(outcome.summary, "exit 0");
    }
}
