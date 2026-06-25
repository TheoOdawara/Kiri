use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use crate::modules::tools::application::command_sandbox::NetworkPolicy;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{RunCommandArgs, parse_args};
use crate::modules::tools::infrastructure::exec::{self, ExecError};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, default_accept_for};
use crate::shared::kernel::tool_call::ToolCall;

/// Bounds for a `run_command` timeout, so the model cannot pin a process slot for hours nor request a
/// sub-second deadline that kills every command. The applied value is clamped into `[1s, 10min]`.
const RUN_COMMAND_MIN_TIMEOUT_MS: u64 = 1_000;
const RUN_COMMAND_MAX_TIMEOUT_MS: u64 = 600_000;

fn effective_timeout_ms(requested: u64) -> u64 {
    requested.clamp(RUN_COMMAND_MIN_TIMEOUT_MS, RUN_COMMAND_MAX_TIMEOUT_MS)
}

pub struct RunCommand {
    plan_blacklist: Arc<[Regex]>,
    net_allow: Arc<[Regex]>,
    require_confinement: bool,
}

impl RunCommand {
    pub fn new(
        plan_blacklist: Arc<[Regex]>,
        net_allow: Arc<[Regex]>,
        require_confinement: bool,
    ) -> Self {
        Self {
            plan_blacklist,
            net_allow,
            require_confinement,
        }
    }

    /// Decide the network stance for a command: allowed when the sandbox's base stance already permits
    /// it or the command matches the dev/package-manager allow-list, otherwise denied. Keeps
    /// `cargo build` / `npm install` fluid while blocking arbitrary outbound calls by default.
    fn network_for(&self, command: &str, base: NetworkPolicy) -> NetworkPolicy {
        if base == NetworkPolicy::Allow
            || self
                .net_allow
                .iter()
                .any(|pattern| pattern.is_match(command))
        {
            NetworkPolicy::Allow
        } else {
            NetworkPolicy::Deny
        }
    }
}

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

        // KIRI_SANDBOX=require: refuse to run an arbitrary shell command unconfined rather than fall
        // back to the path-policy + confirmation layers alone.
        if self.require_confinement && !sandbox.confiner().supports_confinement() {
            return ToolOutcome::Error(
                "OS command sandbox unavailable on this platform; refusing to run unconfined \
                 (KIRI_SANDBOX=require)"
                    .to_string(),
            );
        }

        let cwd = match sandbox.resolve_existing(&args.cwd) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        let network = self.network_for(&args.command, sandbox.network());
        let result = match exec::run_shell(
            &args.command,
            Some(&cwd),
            Duration::from_millis(effective_timeout_ms(args.timeout_ms)),
            sandbox.confiner(),
            &sandbox.command_policy(network, &[&cwd]),
        )
        .await
        {
            Ok(result) => result,
            Err(ExecError::Timeout(ms)) => {
                return ToolOutcome::Error(format!("command timed out after {ms}ms"));
            }
            Err(ExecError::Spawn(error)) => {
                return ToolOutcome::Error(format!("failed to spawn command: {error}"));
            }
        };

        let content = exec::capped_combined(&result);
        let status_str = match result.exit_code {
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

    fn is_plannable(&self) -> bool {
        true
    }

    fn confirm_in_auto(&self) -> bool {
        true
    }

    fn plan_check(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        let args: RunCommandArgs = parse_args(call).ok()?;
        for pattern in self.plan_blacklist.iter() {
            if pattern.is_match(&args.command) {
                return Some(format!(
                    "blocked in plan mode: command matches '{}'",
                    pattern.as_str()
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::fs::default_fs_tools;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::tool_call::FunctionCall;
    use regex::Regex;
    use serde_json::json;
    use std::fs;
    use std::sync::Arc;

    use tempfile::TempDir;

    fn sandbox(dir: &TempDir) -> Sandbox {
        Sandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap()
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
        ToolRegistry::new(default_fs_tools(
            Arc::from(Vec::<Regex>::new()),
            Arc::from(Vec::<Regex>::new()),
            false,
        ))
    }

    use crate::modules::tools::application::registry::ToolRegistry;

    #[test]
    fn effective_timeout_ms_clamps_into_range() {
        assert_eq!(effective_timeout_ms(0), 1_000);
        assert_eq!(effective_timeout_ms(500), 1_000);
        assert_eq!(effective_timeout_ms(30_000), 30_000);
        assert_eq!(effective_timeout_ms(u64::MAX), RUN_COMMAND_MAX_TIMEOUT_MS);
    }

    #[tokio::test]
    async fn run_command_simple() {
        let dir = TempDir::new().unwrap();
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
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("f.txt"), b"content").unwrap();
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
        let dir = TempDir::new().unwrap();
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
        let dir = TempDir::new().unwrap();
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
        let dir = TempDir::new().unwrap();
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
        let dir = TempDir::new().unwrap();
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
                assert!(text.len() <= exec::EXEC_MAX_BYTES + 200);
            }
            other => panic!("expected truncated Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_command_timeout() {
        let dir = TempDir::new().unwrap();
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

    // End-to-end confinement proof through the real tool path: a Sandbox carrying the macOS Seatbelt
    // adapter must stop run_command from writing outside the workspace, while in-jail work still runs.
    #[cfg(target_os = "macos")]
    fn confined_sandbox(dir: &TempDir) -> Sandbox {
        use crate::modules::tools::infrastructure::confine::macos::MacosSeatbelt;
        Sandbox::with_confinement(
            dir.path(),
            SensitiveMatcher::empty(),
            Arc::new(MacosSeatbelt),
            NetworkPolicy::Deny,
            Arc::from(Vec::new()),
            Arc::from(Vec::new()),
        )
        .unwrap()
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn confined_run_command_cannot_write_outside_root() {
        use crate::modules::tools::infrastructure::confine::macos::MacosSeatbelt;
        if MacosSeatbelt::detect().is_none() {
            return; // sandbox-exec unavailable on this host
        }
        let dir = TempDir::new().unwrap();
        let sb = confined_sandbox(&dir);
        let reg = registry();
        let probe = format!(
            "{}/kiri-sbx-must-not-exist-{}",
            std::env::var("HOME").unwrap(),
            std::process::id()
        );
        let _ = fs::remove_file(&probe);
        let cmd = format!("echo leaked > '{probe}' 2>&1; echo done");
        let _ = reg
            .execute(&sb, &call("run_command", json!({ "command": cmd })))
            .await;
        let leaked = std::path::Path::new(&probe).exists();
        let _ = fs::remove_file(&probe);
        assert!(
            !leaked,
            "a confined run_command must not be able to write outside the workspace root"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn confined_run_command_still_works_inside_root() {
        use crate::modules::tools::infrastructure::confine::macos::MacosSeatbelt;
        if MacosSeatbelt::detect().is_none() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let sb = confined_sandbox(&dir);
        let reg = registry();
        let outcome = reg
            .execute(
                &sb,
                &call(
                    "run_command",
                    json!({ "command": "echo hi > inside.txt && cat inside.txt" }),
                ),
            )
            .await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(
                    text.contains("hi"),
                    "confinement must not break in-jail work: {text}"
                )
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }
}
