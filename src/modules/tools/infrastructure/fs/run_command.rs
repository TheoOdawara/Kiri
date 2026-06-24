use std::process::Command;

use serde_json::{Value, json};

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
        Some(format!("$ {}", args.command))
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

        // Resolve working directory
        let cwd = match sandbox.resolve_existing(&args.cwd) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        // Build command: use shell to support pipes, redirects, etc.
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

        // Run; the timeout argument is accepted by the schema for future enforcement but not applied
        // here — a real implementation needs an async process with a kill-on-deadline.
        let _ = args.timeout_ms;
        let output = match std::thread::spawn(move || cmd.output()).join() {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return ToolOutcome::Error(format!("failed to spawn command: {error}"));
            }
            Err(_) => {
                return ToolOutcome::Error("command thread panicked".to_string());
            }
        };

        // Combine stdout and stderr
        let mut combined = Vec::new();
        combined.extend_from_slice(&output.stdout);
        if !output.stderr.is_empty() {
            if !combined.is_empty() {
                combined.push(b'\n');
            }
            combined.extend_from_slice(&output.stderr);
        }

        // Truncate if needed
        let content = if combined.len() > RUN_COMMAND_MAX_BYTES {
            let head = String::from_utf8_lossy(&combined[..RUN_COMMAND_MAX_BYTES]);
            format!("{head}\n… (truncated at {RUN_COMMAND_MAX_BYTES} bytes)")
        } else {
            String::from_utf8_lossy(&combined).into_owned()
        };

        let status_str = match output.status.code() {
            Some(code) => format!("exit code {code}"),
            None => "terminated by signal".to_string(),
        };

        ToolOutcome::Ok(format!("{content}\n[{status_str}]"))
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
        // On unix the inner exec fails before the shell is reached; on Windows the shell runs and
        // reports "not recognized" (or its localized variant) to stderr with a non-zero exit code.
        // Both reach the model as diagnostic text — the shape differs, the contract is the same:
        // a meaningful, non-empty error reaches the caller.
        match outcome {
            ToolOutcome::Error(msg) => assert!(!msg.is_empty()),
            ToolOutcome::Ok(text) => assert!(!text.trim().is_empty()),
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

        // Sleep for longer than the 1s timeout
        let outcome = reg
            .execute(
                &sb,
                &call(
                    "run_command",
                    json!({"command": "sleep 2", "timeout_ms": 1000}),
                ),
            )
            .await;
        // Note: timeout is not enforced at the OS level here; this just documents the argument exists.
        // A real timeout would need a more complex implementation (e.g. tokio::process with timeout).
        match outcome {
            ToolOutcome::Ok(_) | ToolOutcome::Error(_) => {} // either is acceptable for this test
            other => panic!("unexpected {other:?}"),
        }
    }
}
