//! The sanctioned site for the `hooks` context's process I/O (ADR 0021). Routes through
//! `tools::infrastructure::exec::run_shell` so a hook cannot bypass the sandbox's OS-level confinement.

use std::time::Duration;

use crate::modules::extensions::domain::resource::Hook;
use crate::modules::hooks::application::hook_runner::{HookOutcome, HookRunner};
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::infrastructure::exec::{self, ExecError, capped_combined};
use crate::shared::kernel::sandbox::NetworkPolicy;

/// Hooks are auxiliary and notice-only, so they get a far tighter bound than a model-driven
/// `run_command` call.
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// Network-denied by default: a hook is a notification point, not an integration — that is what MCP is for.
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

    // Unix shell semantics (`echo` output shape); Windows (`cmd /C`) is not a v1 target.
    #[cfg(unix)]
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

    // `true` is a Unix builtin/binary; `cmd /C true` fails on Windows, which is not a v1 target.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_with_no_output_summarizes_the_exit_code() {
        let dir = TempDir::new().unwrap();
        let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ShellHookRunner.run(&sandbox, &hook("true")).await;
        assert!(outcome.ok);
        assert_eq!(outcome.summary, "exit 0");
    }

    /// `STDERR_MARKER` belongs to `exec::capped_combined_marking_stderr`; this runner calls the plain
    /// `capped_combined` and must report the command's own stderr text, never the marker line.
    #[tokio::test]
    async fn a_command_with_only_stderr_output_reports_the_real_error_not_a_marker() {
        let dir = TempDir::new().unwrap();
        let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let script = script(
            "echo boom 1>&2; exit 1",
            "[Console]::Error.WriteLine('boom'); exit 1",
        );
        let outcome = ShellHookRunner.run(&sandbox, &hook(script)).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.summary, "boom");
    }

    fn script(unix: &'static str, windows: &'static str) -> &'static str {
        if cfg!(windows) { windows } else { unix }
    }
}
