//! The single place the tools layer spawns a child process. Every file tool translates its validated
//! arguments into a terminal command and runs it here; `run_command` runs an arbitrary model command
//! through the platform shell. Centralizes the process plumbing — piped stdio, concurrent stdin
//! writing, a timeout that kills the child, and the 64 KiB output cap — so no tool reimplements it.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};

/// Combined stdout/stderr is truncated at this many bytes before it reaches the model.
pub const EXEC_MAX_BYTES: usize = 64 * 1024;

/// The bound for a file tool's command. `run_command` overrides it with its own configurable timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The captured result of a finished subprocess. `stdout`/`stderr` are raw and uncapped — each caller
/// decides how to combine and bound them (`run_command` caps the combined stream; `read_file` applies
/// its own read cap to `stdout`).
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}

impl ExecResult {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// The child's stderr as text, trimmed — the detail interpolated into a tool's error message.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Why a command did not produce a result: the shell/binary failed to launch, or it ran past the bound.
#[derive(Debug)]
pub enum ExecError {
    Spawn(String),
    Timeout(u64),
}

impl ExecError {
    pub fn message(&self) -> String {
        match self {
            ExecError::Spawn(error) => format!("failed to run command: {error}"),
            ExecError::Timeout(ms) => format!("command timed out after {ms}ms"),
        }
    }
}

/// Run an explicit argv (no shell). The validated absolute path is passed as its own OS-level argument,
/// so there is no shell quoting or word-splitting — the workhorse for the translated file tools.
/// `stdin` feeds raw bytes to the child (e.g. `tee` writing a file's content); `env` sets process
/// environment without interpolating values into the command (e.g. `edit_file`'s old/new strings).
pub async fn run_argv(
    argv: &[&OsStr],
    cwd: Option<&Path>,
    stdin: Option<&[u8]>,
    env: &[(&str, &OsStr)],
    timeout: Duration,
    confiner: &dyn CommandSandbox,
    policy: &SandboxPolicy,
) -> Result<ExecResult, ExecError> {
    let (program, rest) = argv.split_first().expect("argv must not be empty");
    let mut cmd = Command::new(program);
    cmd.args(rest);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    let cmd = confiner
        .confine(cmd, policy)
        .map_err(|error| ExecError::Spawn(error.to_string()))?;
    run(cmd, stdin, timeout).await
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
    run(cmd, None, timeout).await
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

/// Spawn, feed stdin while draining stdout/stderr (so a child echoing its input — e.g. `tee` — cannot
/// deadlock against a full pipe), and bound the whole thing by `timeout`. `kill_on_drop` ensures a
/// timed-out child is killed when the future is dropped. Stdin is `null` when no input is supplied, so
/// a command that would otherwise prompt (`rm` on a write-protected file) sees EOF instead of hanging.
async fn run(
    mut cmd: Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<ExecResult, ExecError> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let ms = timeout.as_millis() as u64;
    let fut = async {
        let mut child = cmd
            .spawn()
            .map_err(|error| ExecError::Spawn(error.to_string()))?;
        let sink = child.stdin.take();
        let writer = async move {
            // Write the payload, then drop `sink` to close the pipe so the child reads EOF and finishes.
            if let Some(mut sink) = sink
                && let Some(bytes) = stdin
            {
                let _ = sink.write_all(bytes).await;
            }
        };
        let (_, output) = tokio::join!(writer, child.wait_with_output());
        let output = output.map_err(|error| ExecError::Spawn(error.to_string()))?;
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::modules::tools::application::command_sandbox::{NetworkPolicy, SandboxPolicy};
    use crate::modules::tools::infrastructure::confine::noop::NoConfinement;

    fn policy() -> SandboxPolicy {
        SandboxPolicy {
            root: std::path::PathBuf::from("/"),
            network: NetworkPolicy::Allow,
            extra_ro: Vec::new(),
            extra_rw: Vec::new(),
        }
    }

    #[tokio::test]
    async fn run_argv_captures_stdout_and_exit_code() {
        let result = run_argv(
            &[OsStr::new("printf"), OsStr::new("hi")],
            None,
            None,
            &[],
            DEFAULT_TIMEOUT,
            &NoConfinement,
            &policy(),
        )
        .await
        .expect("printf runs");
        assert_eq!(result.stdout, b"hi");
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn run_argv_feeds_stdin() {
        // `cat` echoes stdin to stdout — exercises the concurrent stdin-writer / stdout-drainer path.
        let result = run_argv(
            &[OsStr::new("cat")],
            None,
            Some(b"piped payload"),
            &[],
            DEFAULT_TIMEOUT,
            &NoConfinement,
            &policy(),
        )
        .await
        .expect("cat runs");
        assert_eq!(result.stdout, b"piped payload");
    }

    #[tokio::test]
    async fn run_argv_passes_env_without_interpolation() {
        let result = run_argv(
            &[
                OsStr::new("sh"),
                OsStr::new("-c"),
                OsStr::new("printf %s \"$KIRI_TEST\""),
            ],
            None,
            None,
            &[("KIRI_TEST", OsStr::new("$(whoami) literal"))],
            DEFAULT_TIMEOUT,
            &NoConfinement,
            &policy(),
        )
        .await
        .expect("sh runs");
        assert_eq!(result.stdout, b"$(whoami) literal");
    }

    #[tokio::test]
    async fn run_shell_times_out_and_reports_ms() {
        let error = run_shell(
            "sleep 5",
            None,
            Duration::from_millis(100),
            &NoConfinement,
            &policy(),
        )
        .await
        .err()
        .expect("expected a timeout");
        assert!(error.message().contains("timed out"));
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

    #[tokio::test]
    async fn run_argv_reports_spawn_failure_for_missing_binary() {
        let error = run_argv(
            &[OsStr::new("kiri_no_such_binary_zzz")],
            None,
            None,
            &[],
            DEFAULT_TIMEOUT,
            &NoConfinement,
            &policy(),
        )
        .await
        .err()
        .expect("missing binary fails to spawn");
        assert!(matches!(error, ExecError::Spawn(_)));
    }
}
