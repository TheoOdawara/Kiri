//! The single place the tools layer spawns a child process — `run_command` runs an arbitrary model
//! command through the platform shell (the file tools operate the filesystem natively, via `std::fs`,
//! and never reach this module). Centralizes the process plumbing — piped stdio, a timeout that kills
//! the child, and the 64 KiB output cap — so `run_command` does not reimplement it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};

/// Combined stdout/stderr is truncated at this many bytes before it reaches the model.
pub const EXEC_MAX_BYTES: usize = 64 * 1024;

/// The bound for a file tool's command. `run_command` overrides it with its own configurable timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The captured result of a finished subprocess. `stdout`/`stderr` are raw and uncapped; `run_command`
/// caps the combined stream via `capped_combined`.
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}

/// Why a command did not produce a result: the shell failed to launch, or it ran past the bound.
#[derive(Debug)]
pub enum ExecError {
    Spawn(String),
    Timeout(u64),
}

/// Run a command line through the platform shell (`sh -c` / `cmd /C`). Used only by `run_command`,
/// which executes an arbitrary command the model supplies.
pub async fn run_shell(
    script: &str,
    cwd: Option<&Path>,
    timeout: Duration,
    confiner: &dyn CommandSandbox,
    policy: &SandboxPolicy,
) -> Result<ExecResult, ExecError> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", script]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let cmd = confiner
        .confine(cmd, policy)
        .map_err(|error| ExecError::Spawn(error.to_string()))?;
    run(cmd, timeout).await
}

/// Combine `stdout` then `stderr` (as `run_command` reports them) and truncate at `EXEC_MAX_BYTES`.
pub fn capped_combined(result: &ExecResult) -> String {
    let mut combined = Vec::new();
    combined.extend_from_slice(&result.stdout);
    if !result.stderr.is_empty() {
        if !combined.is_empty() {
            combined.push(b'\n');
        }
        combined.extend_from_slice(&result.stderr);
    }
    if combined.len() > EXEC_MAX_BYTES {
        let head = String::from_utf8_lossy(&combined[..EXEC_MAX_BYTES]);
        format!("{head}\n… (truncated at {EXEC_MAX_BYTES} bytes)")
    } else {
        String::from_utf8_lossy(&combined).into_owned()
    }
}

/// Spawn, capture stdout/stderr, and bound the whole thing by `timeout`. `kill_on_drop` ensures a
/// timed-out child is killed when the future is dropped. Stdin is always `null` — `run_command` is the
/// sole caller and never feeds input, so a command that would otherwise prompt (e.g. on a
/// write-protected target) sees EOF instead of hanging.
async fn run(mut cmd: Command, timeout: Duration) -> Result<ExecResult, ExecError> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let ms = timeout.as_millis() as u64;
    let fut = async {
        let output = cmd
            .spawn()
            .map_err(|error| ExecError::Spawn(error.to_string()))?
            .wait_with_output()
            .await
            .map_err(|error| ExecError::Spawn(error.to_string()))?;
        Ok(ExecResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.status.code(),
        })
    };

    match tokio::time::timeout(timeout, fut).await {
        Ok(result) => result,
        Err(_) => Err(ExecError::Timeout(ms)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::application::command_sandbox::SandboxPolicy;
    use crate::modules::tools::infrastructure::confine::noop::NoConfinement;
    use crate::shared::kernel::sandbox::NetworkPolicy;

    fn policy() -> SandboxPolicy {
        SandboxPolicy {
            root: std::path::PathBuf::from("."),
            network: NetworkPolicy::Allow,
            extra_ro: Vec::new(),
            extra_rw: Vec::new(),
        }
    }

    /// `sh`'s builtins differ from `cmd`'s, so each test picks its own per-platform script; `run_shell`
    /// itself already branches the same way in production.
    fn script(unix: &'static str, windows: &'static str) -> &'static str {
        if cfg!(windows) { windows } else { unix }
    }

    #[tokio::test]
    async fn run_shell_captures_stdout_and_exit_code() {
        let result = run_shell(
            script("printf hi", "echo hi"),
            None,
            DEFAULT_TIMEOUT,
            &NoConfinement,
            &policy(),
        )
        .await
        .expect("script runs");
        // `cmd /C echo` appends a CRLF that `printf` (no implicit newline) does not; trim so the
        // assertion is platform-independent.
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "hi");
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn run_shell_times_out_and_reports_ms() {
        let error = run_shell(
            script("sleep 5", "ping -n 6 127.0.0.1 >NUL"),
            None,
            Duration::from_millis(100),
            &NoConfinement,
            &policy(),
        )
        .await
        .err()
        .expect("expected a timeout");
        assert!(matches!(error, ExecError::Timeout(_)));
    }

    #[tokio::test]
    async fn run_shell_reports_a_failing_command_by_exit_code_not_spawn_error() {
        // The shell itself always launches; a failing inner command is a normal non-zero exit, never
        // `ExecError::Spawn` (that variant is reserved for the shell/binary failing to launch at all).
        let result = run_shell(
            script("exit 3", "exit 3"),
            None,
            DEFAULT_TIMEOUT,
            &NoConfinement,
            &policy(),
        )
        .await
        .expect("the shell launches even though the inner command fails");
        assert_eq!(result.exit_code, Some(3));
    }

    #[tokio::test]
    async fn capped_combined_truncates_large_output() {
        let result = ExecResult {
            stdout: vec![b'a'; EXEC_MAX_BYTES + 500],
            stderr: Vec::new(),
            exit_code: Some(0),
        };
        let text = capped_combined(&result);
        assert!(text.contains("truncated at"));
        assert!(text.len() <= EXEC_MAX_BYTES + 200);
    }
}
