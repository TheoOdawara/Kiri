use std::time::Duration;

use serde_json::{Value, json};
use tokio::process::Command;

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{RunCommandArgs, parse_args};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, default_accept_for};
use crate::shared::kernel::tool_call::ToolCall;

const RUN_COMMAND_MAX_BYTES: usize = 64 * 1024;

pub struct RunCommand;

#[async_trait::async_trait(?Send)]
impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Run a shell command and return its combined stdout/stderr output. The command runs in the \
             specified working directory (relative to workspace root, or absolute). Output is truncated \
             at 64 KiB. A timeout (default 30s) prevents runaway commands.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["command"],
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory (relative to workspace root, or absolute). Defaults to workspace root.",
                        "default": "."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds. Defaults to 30000.",
                        "default": 30000,
                        "minimum": 1000
                    }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        let args: RunCommandArgs = parse_args(call).ok()?;
        if cfg!(windows) {
            Some(format!("$ cmd /C \"{}\"", args.command))
        } else {
            Some(format!("$ sh -c '{}'", args.command))
        }
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let args: RunCommandArgs = parse_args(call).ok()?;
        let cmd = self.command_line(sandbox, call)?;
        let action = format!("Executar comando no shell. Aprova executar: {cmd}?");
        let default_accept = default_accept_for(&args.cwd);
        Some(confirm(action, default_accept))
    }

    async fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: RunCommandArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };

        let cwd = match sandbox.resolve_existing(&args.cwd) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", &args.command]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", &args.command]);
            c
        };
        cmd.current_dir(&cwd);
        // Kill the child if the timeout future is dropped, so a timed-out command doesn't
        // keep running in the background after the tool returns.
        cmd.kill_on_drop(true);

        let timeout = Duration::from_millis(args.timeout_ms);
        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return ToolOutcome::Error(format!("failed to spawn command: {error}"));
            }
            Err(_) => {
                return ToolOutcome::Error(format!(
                    "command timed out after {}ms",
                    args.timeout_ms
                ));
            }
        };

        let mut combined = Vec::new();
        combined.extend_from_slice(&output.stdout);
        if !output.stderr.is_empty() {
            if !combined.is_empty() {
                combined.push(b'\n');
            }
            combined.extend_from_slice(&output.stderr);
        }

        let content = if combined.len() > RUN_COMMAND_MAX_BYTES {
            let head = String::from_utf8_lossy(&combined[..RUN_COMMAND_MAX_BYTES]);
            format!("{head}\n… (truncated at {RUN_COMMAND_MAX_BYTES} bytes)")
        } else {
            String::from_utf8_lossy(&combined).into_owned()
        };

        let status_str = match output.status.code() {
            Some(code) => format!("exit code {code}"),
            None => "terminated (no exit code)".to_string(),
        };

        if content.is_empty() {
            ToolOutcome::Ok(format!("[{status_str}]"))
        } else {
            ToolOutcome::Ok(format!("{content}\n[{status_str}]"))
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::fs::default_fs_tools;
    use crate::shared::kernel::tool_call::FunctionCall;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            let pid = std::process::id();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            path.push(format!("t-cli-run-cmd-{tag}-{pid}-{n}"));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sandbox(dir: &TempDir) -> Sandbox {
        Sandbox::new(&dir.path).unwrap()
    }

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    fn registry() -> ToolRegistry {
        ToolRegistry::new(default_fs_tools())
    }

    use crate::modules::tools::application::registry::ToolRegistry;

    #[tokio::test]
    async fn run_command_simple() {
        let dir = TempDir::new("simple");
        let sb = sandbox(&dir);
        let reg = registry();

        let outcome = reg
            .execute(&sb, &call("run_command", json!({"command": "echo hello"})))
            .await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(text.contains("hello"));
                assert!(text.contains("exit code 0"));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_command_with_cwd() {
        let dir = TempDir::new("cwd");
        fs::create_dir(dir.path.join("sub")).unwrap();
        fs::write(dir.path.join("sub").join("f.txt"), b"content").unwrap();
        let sb = sandbox(&dir);
        let reg = registry();

        // `cat` is unix; `type` is the cmd.exe equivalent. The test only cares that the file's
        // content reaches the model — the read primitive is incidental.
        #[cfg(unix)]
        let read_cmd = "cat f.txt";
        #[cfg(windows)]
        let read_cmd = "type f.txt";

        let outcome = reg
            .execute(
                &sb,
                &call("run_command", json!({"command": read_cmd, "cwd": "sub"})),
            )
            .await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(text.contains("content"));
                assert!(text.contains("exit code 0"));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_command_nonexistent_cwd_returns_error() {
        let dir = TempDir::new("bad-cwd");
        let sb = sandbox(&dir);
        let reg = registry();

        let outcome = reg
            .execute(
                &sb,
                &call("run_command", json!({"command": "echo x", "cwd": "nope"})),
            )
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[tokio::test]
    async fn run_command_failure_exit_code() {
        let dir = TempDir::new("fail");
        let sb = sandbox(&dir);
        let reg = registry();

        let outcome = reg
            .execute(&sb, &call("run_command", json!({"command": "exit 42"})))
            .await;
        match outcome {
            ToolOutcome::Ok(text) => assert!(text.contains("exit code 42")),
            other => panic!("expected Ok with exit code, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_command_not_found() {
        let dir = TempDir::new("notfound");
        let sb = sandbox(&dir);
        let reg = registry();

        let outcome = reg
            .execute(
                &sb,
                &call(
                    "run_command",
                    json!({"command": "this_command_does_not_exist_xyz"}),
                ),
            )
            .await;
        // The shell runs and reports the missing command to stderr with a non-zero exit code.
        // The tool returns Ok with the diagnostic text — the exit code is the signal, not the
        // outcome variant. A spawn failure (shell itself missing) returns Error.
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(
                    !text.contains("exit code 0"),
                    "expected non-zero exit code, got: {text}"
                );
            }
            ToolOutcome::Error(msg) => assert!(!msg.is_empty(), "expected non-empty error"),
            ToolOutcome::Declined => panic!("unexpected Declined"),
        }
    }

    #[tokio::test]
    async fn run_command_truncates_large_output() {
        let dir = TempDir::new("truncate");
        let sb = sandbox(&dir);
        let reg = registry();

        // Generate enough output to exceed RUN_COMMAND_MAX_BYTES. The shell loop syntax differs
        // between bash and cmd — the assertion only cares about the truncation marker.
        #[cfg(unix)]
        let spam = "for i in $(seq 1 50000); do echo $i; done";
        #[cfg(windows)]
        let spam = "for /L %i in (1,1,50000) do @echo %i";

        let outcome = reg
            .execute(&sb, &call("run_command", json!({"command": spam})))
            .await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(text.contains("truncated at"));
                assert!(text.len() <= RUN_COMMAND_MAX_BYTES + 200);
            }
            other => panic!("expected truncated Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_command_timeout() {
        let dir = TempDir::new("timeout");
        let sb = sandbox(&dir);
        let reg = registry();

        // A command that takes ~1s, with a 100ms timeout — the tool must kill it and
        // return a timeout error. Platform-specific because `sleep` is not on cmd.exe.
        #[cfg(unix)]
        let slow = "sleep 1";
        #[cfg(windows)]
        let slow = "ping -n 2 127.0.0.1 > nul";

        let outcome = reg
            .execute(
                &sb,
                &call("run_command", json!({"command": slow, "timeout_ms": 100})),
            )
            .await;
        match outcome {
            ToolOutcome::Error(msg) => {
                assert!(
                    msg.contains("timed out"),
                    "expected timeout error, got: {msg}"
                )
            }
            other => panic!("expected timeout Error, got {other:?}"),
        }
    }
}
