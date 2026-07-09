//! The single place the tools layer spawns a child process — `run_command` runs an arbitrary model
//! command through the platform shell (the file tools operate the filesystem natively, via `std::fs`,
//! and never reach this module). Centralizes the process plumbing — piped stdio, a timeout that kills
//! the **whole process tree** (Windows Job Object / Unix process group via `process-wrap`), and the
//! 64 KiB streaming output cap — so `run_command` does not reimplement it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};

/// Combined stdout/stderr is truncated at this many bytes before it reaches the model.
pub const EXEC_MAX_BYTES: usize = 64 * 1024;

/// The line [`capped_combined_marking_stderr`] inserts between stdout and stderr when both are
/// non-empty — a stable, human-legible boundary both the model (it gets this text as the tool result) and
/// the TUI (issue #8a: distinguish stderr and always show it in full) can recognize, without changing
/// `ToolOutcome`'s plain `String` shape or threading a structured result through every `Tool` impl for the
/// sake of the one caller that wants it split. `ShellHookRunner` keeps using the plain `capped_combined` —
/// it parses only the first output line for its notice summary, so a literal marker line would corrupt
/// that summary for any hook whose command writes only to stderr.
pub const STDERR_MARKER: &str = "--- stderr ---";

/// Non-secret env vars a spawned command needs to resolve/run typical shell scripts and dev/package
/// tools (cargo, npm, git, …). Re-added after `env_clear()` so nothing else — provider API keys and
/// other credentials the harness process holds — leaks into a model-supplied command; a compromised or
/// careless command must not be able to read them back via `env`/`printenv` (issues #25/#49; ADR 0026;
/// mirrors the same pattern already used for MCP server children, `rmcp_client.rs`). Both `run_command`
/// and hooks route through this one function, so scrubbing here closes both surfaces at once.
const INHERITED_ENV_VARS: &[&str] = &[
    "PATH",
    // Windows: after env scrub, PowerShell/cmd resolve `ping` → `ping.exe` via PATHEXT. Without it,
    // `run_command` timeout tests (and any bare tool name) fail with "not recognized" even when the
    // binary is on PATH.
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "SystemRoot",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
];

/// The bound for a file tool's command. `run_command` overrides it with its own configurable timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Clear `cmd`'s environment and re-apply only [`INHERITED_ENV_VARS`], resolved through `lookup`. The
/// lookup is injected (mirroring `provider::factory::resolve_credential_with_env`) so this is unit-testable
/// without mutating real process env — edition-2024 `std::env::set_var` is `unsafe`, and this crate
/// forbids `unsafe` code.
fn scrub_env(cmd: &mut Command, lookup: impl Fn(&str) -> Option<String>) {
    cmd.env_clear();
    for key in INHERITED_ENV_VARS {
        if let Some(value) = lookup(key) {
            cmd.env(key, value);
        }
    }
}

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
        let mut c = Command::new("pwsh");
        c.args(["-Command", script]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    scrub_env(&mut cmd, |key| std::env::var(key).ok());
    let cmd = confiner
        .confine(cmd, policy)
        .map_err(|error| ExecError::Spawn(error.to_string()))?;
    match run(cmd, timeout).await {
        Ok(res) => Ok(res),
        Err(ExecError::Spawn(e)) => {
            if cfg!(windows)
                && (e.contains("not found")
                    || e.contains("os error 2")
                    || e.contains("entity not found"))
            {
                Err(ExecError::Spawn(
                    "PowerShell 7 (pwsh) is required on Windows but was not found on PATH."
                        .to_string(),
                ))
            } else {
                Err(ExecError::Spawn(e))
            }
        }
        Err(e) => Err(e),
    }
}

/// Combine `stdout` then `stderr` with a plain newline and truncate at `EXEC_MAX_BYTES`. Used by
/// `ShellHookRunner`, whose notice summary is just the first output line — inserting a marker here would
/// corrupt that summary for a hook whose command writes only to stderr.
pub fn capped_combined(result: &ExecResult) -> String {
    let mut combined = Vec::new();
    combined.extend_from_slice(&result.stdout);
    if !result.stderr.is_empty() {
        if !combined.is_empty() {
            combined.push(b'\n');
        }
        combined.extend_from_slice(&result.stderr);
    }
    truncate_at_cap(combined)
}

/// Combine `stdout` then `stderr`, setting stderr off with [`STDERR_MARKER`] on its own line when it is
/// non-empty, and truncate at `EXEC_MAX_BYTES`. Used by `run_command` alone, whose TUI rendering and model
/// message both benefit from telling the two streams apart (issue #8a) — see `STDERR_MARKER`'s doc comment
/// for why this is a separate function rather than changing `capped_combined` for every caller.
pub fn capped_combined_marking_stderr(result: &ExecResult) -> String {
    let mut combined = Vec::new();
    combined.extend_from_slice(&result.stdout);
    if !result.stderr.is_empty() {
        combined.push(b'\n');
        combined.extend_from_slice(STDERR_MARKER.as_bytes());
        combined.push(b'\n');
        combined.extend_from_slice(&result.stderr);
    }
    truncate_at_cap(combined)
}

/// Truncate `combined` at [`EXEC_MAX_BYTES`], the shared byte-cap logic behind both `capped_combined`
/// variants above.
fn truncate_at_cap(combined: Vec<u8>) -> String {
    truncate_tool_output_bytes(&combined)
}

/// Truncate model/tool-facing text at [`EXEC_MAX_BYTES`] with a stable marker. Shared by shell output
/// and MCP tool results so every path that feeds the transcript applies the same bound.
pub fn truncate_tool_output(text: &str) -> String {
    truncate_tool_output_bytes(text.as_bytes())
}

fn truncate_tool_output_bytes(bytes: &[u8]) -> String {
    if bytes.len() > EXEC_MAX_BYTES {
        let head = String::from_utf8_lossy(&bytes[..EXEC_MAX_BYTES]);
        format!("{head}\n… (truncated at {EXEC_MAX_BYTES} bytes)")
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Spawn, capture stdout/stderr with a **streaming** byte cap, and bound the whole thing by `timeout`.
///
/// Process-tree kill (F-BUG-002 / #42): after OS confine decorates the command, the spawn is wrapped
/// with `process-wrap` so timeout / drop kills the **whole tree**:
/// - **Windows:** Job Object (`TerminateJobObject` on kill) — grandchildren of `pwsh` die too.
/// - **Unix:** process group leader — `killpg` on the group.
///
/// Stdin is always `null` — `run_command` is the sole caller and never feeds input, so a command that
/// would otherwise prompt (e.g. on a write-protected target) sees EOF instead of hanging.
async fn run(mut cmd: Command, timeout: Duration) -> Result<ExecResult, ExecError> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // KillOnDrop must be the process-wrap shim (not Command::kill_on_drop alone) so JobObject can
    // read the flag and enable KILL_ON_JOB_CLOSE.
    let mut wrap = CommandWrap::from(cmd);
    wrap.wrap(KillOnDrop);
    #[cfg(windows)]
    wrap.wrap(JobObject);
    #[cfg(unix)]
    wrap.wrap(ProcessGroup::leader());

    let ms = timeout.as_millis() as u64;
    let mut child = wrap
        .spawn()
        .map_err(|error| ExecError::Spawn(error.to_string()))?;

    let collect = collect_output(child.as_mut());
    match tokio::time::timeout(timeout, collect).await {
        Ok(result) => result,
        Err(_) => {
            // Job Object / process group kill — best-effort; Timeout is returned either way.
            // `start_kill` is the tree-wide signal (TerminateJobObject / killpg); `wait` reaps.
            // (ChildWrapper::kill returns an unpinned `Box<dyn Future>`, so call the pieces here.)
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(ExecError::Timeout(ms))
        }
    }
}

/// Drain stdout/stderr with a hard per-stream cap of [`EXEC_MAX_BYTES`], discarding the remainder so the
/// child never blocks on a full pipe. Bounds harness RSS even when the model-facing combined cap is
/// applied later via [`capped_combined`].
async fn collect_output(child: &mut dyn ChildWrapper) -> Result<ExecResult, ExecError> {
    let mut stdout_pipe = child
        .stdout()
        .take()
        .ok_or_else(|| ExecError::Spawn("child stdout pipe missing".to_string()))?;
    let mut stderr_pipe = child
        .stderr()
        .take()
        .ok_or_else(|| ExecError::Spawn("child stderr pipe missing".to_string()))?;

    let stdout_task = async {
        let mut buf = Vec::new();
        read_capped_stream(&mut stdout_pipe, &mut buf, EXEC_MAX_BYTES).await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    };
    let stderr_task = async {
        let mut buf = Vec::new();
        read_capped_stream(&mut stderr_pipe, &mut buf, EXEC_MAX_BYTES).await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    };

    let (stdout_res, stderr_res) = tokio::join!(stdout_task, stderr_task);
    let stdout = stdout_res.map_err(|error| ExecError::Spawn(error.to_string()))?;
    let stderr = stderr_res.map_err(|error| ExecError::Spawn(error.to_string()))?;
    let status = child
        .wait()
        .await
        .map_err(|error| ExecError::Spawn(error.to_string()))?;
    Ok(ExecResult {
        stdout,
        stderr,
        exit_code: status.code(),
    })
}

/// Read from `reader` until EOF, appending at most `cap` bytes and discarding the rest.
async fn read_capped_stream(
    reader: &mut (impl AsyncReadExt + Unpin),
    dest: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<()> {
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if dest.len() < cap {
            let take = (cap - dest.len()).min(n);
            dest.extend_from_slice(&chunk[..take]);
        }
    }
    Ok(())
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

    #[test]
    fn scrub_env_keeps_only_the_allowlist() {
        let mut cmd = Command::new("true");
        scrub_env(&mut cmd, |key| match key {
            "PATH" => Some("/usr/bin".to_string()),
            // Not in INHERITED_ENV_VARS — must be dropped, not carried into the child.
            "NVIDIA_API_KEY" => Some("should-not-leak".to_string()),
            _ => None,
        });
        let keys: Vec<_> = cmd
            .as_std()
            .get_envs()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            keys,
            vec!["PATH".to_string()],
            "only the allowlisted var the lookup provided must survive: {keys:?}"
        );
    }

    /// Vars a POSIX shell synthesizes itself on startup, from nothing — never inherited from the parent,
    /// so scrubbing the parent env cannot leak anything through them. `PWD` derives from the process's
    /// cwd, `SHLVL` defaults to 1 with no inherited value, `_` is `sh`'s own "last command" bookkeeping.
    /// Only the `#[cfg(unix)]` scrub test below reads it, so it is unix-gated too (unused on Windows).
    #[cfg(unix)]
    const SHELL_SYNTHESIZED_VARS: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_"];

    // The env allowlist (INHERITED_ENV_VARS) is Unix-oriented; Windows needs OS vars like SYSTEMROOT that
    // are intentionally outside it, so this scrubbing assertion is Unix-only (Windows is not a v1 target).
    #[cfg(unix)]
    #[tokio::test]
    async fn run_shell_scrubs_env_down_to_the_allowlist() {
        // End-to-end: whatever this test process's REAL environment contains (cargo/CI vars, any local
        // secret-shaped var), the spawned child must see nothing outside INHERITED_ENV_VARS (plus the
        // shell's own synthesized vars, which never came from the parent). Lists the child's actual
        // environment via a shell builtin rather than asserting on one specific var — reads ambient env
        // only, never mutates it (`set_var` is `unsafe` in edition 2024; this crate forbids `unsafe`), so
        // this needs no env fixture at all.
        let result = run_shell(
            script("env", "Get-ChildItem Env: | ForEach-Object { $_.Name }"),
            None,
            DEFAULT_TIMEOUT,
            &NoConfinement,
            &policy(),
        )
        .await
        .expect("script runs");
        let output = String::from_utf8_lossy(&result.stdout);
        for line in output.lines() {
            let key = line.split('=').next().unwrap_or(line).trim();
            if key.is_empty() {
                continue;
            }
            assert!(
                INHERITED_ENV_VARS.contains(&key) || SHELL_SYNTHESIZED_VARS.contains(&key),
                "child process saw an env var outside the allowlist: {key} (full env: {output})"
            );
        }
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

    /// Streaming drain must cap in-memory buffers during collection, not only after the process exits.
    #[tokio::test]
    async fn run_shell_caps_output_while_streaming() {
        let oversized = EXEC_MAX_BYTES + 50_000;
        let cmd = if cfg!(windows) {
            format!("$s = 'a' * {oversized}; [Console]::Out.Write($s)")
        } else {
            // Portable: no `dd`/`yes` dependency — Python is available on the CI images used here.
            format!("python3 -c \"import sys; sys.stdout.write('a' * {oversized})\"")
        };
        let result = run_shell(
            &cmd,
            None,
            Duration::from_secs(30),
            &NoConfinement,
            &policy(),
        )
        .await
        .expect("script runs");
        assert!(
            result.stdout.len() <= EXEC_MAX_BYTES,
            "stdout buffer must be capped during drain, got {}",
            result.stdout.len()
        );
    }

    /// F-BUG-002 / #42: timeout must kill the **whole process tree**, not only the direct shell.
    /// A nested long-running grandchild holds an exclusive lock on a marker file; after Timeout the
    /// harness must be able to open that file (proving the grandchild is dead).
    #[tokio::test]
    async fn timeout_kills_grandchild_process() {
        use std::fs::OpenOptions;
        use std::io::Write;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("tree_kill.lock");
        // Pre-create so the path is stable for both platforms' shell quoting.
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)
            .unwrap()
            .write_all(b"init")
            .unwrap();
        let lock = lock_path.to_string_lossy().replace('\\', "\\\\");

        // Nested process so the lock-holder is a grandchild of the job/group root (our outer shell).
        let cmd = if cfg!(windows) {
            // Inner pwsh holds ShareMode.None on the marker; outer sleeps until we kill the job.
            format!(
                "Start-Process -FilePath pwsh -ArgumentList '-NoProfile','-Command', \
                 \"$f=[IO.File]::Open('{lock}','Open','ReadWrite','None'); \
                 Start-Sleep -Seconds 120; $f.Close()\" \
                 -WindowStyle Hidden | Out-Null; Start-Sleep -Seconds 120"
            )
        } else {
            // Inner python holds an exclusive flock; outer sleeps.
            format!(
                "python3 -c \"import fcntl,time;f=open(r'{lock}','a+');fcntl.flock(f,fcntl.LOCK_EX);time.sleep(120)\" & sleep 120"
            )
        };

        let error = run_shell(
            &cmd,
            None,
            Duration::from_millis(400),
            &NoConfinement,
            &policy(),
        )
        .await
        .err()
        .expect("expected a timeout");
        assert!(matches!(error, ExecError::Timeout(_)));

        // Give the OS a moment to reap; then exclusive open must succeed if the tree died.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let reopened = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path);
        assert!(
            reopened.is_ok(),
            "grandchild still holds the lock after timeout — process tree was not killed: {reopened:?}"
        );
    }

    #[test]
    fn capped_combined_marking_stderr_sets_it_off_with_the_marker() {
        let result = ExecResult {
            stdout: b"line one".to_vec(),
            stderr: b"boom".to_vec(),
            exit_code: Some(1),
        };
        let text = capped_combined_marking_stderr(&result);
        assert_eq!(text, format!("line one\n{STDERR_MARKER}\nboom"));
    }

    #[test]
    fn capped_combined_marking_stderr_with_no_stdout_still_marks_stderr() {
        let result = ExecResult {
            stdout: Vec::new(),
            stderr: b"boom".to_vec(),
            exit_code: Some(1),
        };
        let text = capped_combined_marking_stderr(&result);
        assert_eq!(text, format!("\n{STDERR_MARKER}\nboom"));
    }

    #[test]
    fn capped_combined_marking_stderr_with_no_stderr_never_inserts_the_marker() {
        let result = ExecResult {
            stdout: b"line one".to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
        };
        let text = capped_combined_marking_stderr(&result);
        assert_eq!(text, "line one");
        assert!(!text.contains(STDERR_MARKER));
    }

    #[test]
    fn capped_combined_never_inserts_the_marker_even_with_stderr() {
        // ShellHookRunner::first_line parses only this function's first output line for its notice
        // summary — a marker line here would corrupt that summary for a hook whose command writes only
        // to stderr. `capped_combined` (unlike `capped_combined_marking_stderr`) must stay the plain
        // merge every one of its other callers already relies on.
        let result = ExecResult {
            stdout: b"line one".to_vec(),
            stderr: b"boom".to_vec(),
            exit_code: Some(1),
        };
        let text = capped_combined(&result);
        assert_eq!(text, "line one\nboom");
        assert!(!text.contains(STDERR_MARKER));
    }
}
