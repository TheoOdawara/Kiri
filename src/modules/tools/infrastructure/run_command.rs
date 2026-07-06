use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use crate::modules::tools::application::path::is_absolute_target;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, confirm_execute_suffix, function_schema,
};
use crate::modules::tools::infrastructure::args::{
    RUN_COMMAND_DEFAULT_TIMEOUT_MS, RunCommandArgs, parse_args,
};
use crate::modules::tools::infrastructure::exec::{self, ExecError};
use crate::shared::kernel::tool_call::ToolCall;

/// The leading program token of a command (first whitespace-delimited word). The plan-mode allow-list
/// (`plan_check`) is matched against this — the *invoked* program — not any substring of the whole line,
/// so a chained `cargo metadata; curl …` cannot inherit auto-run eligibility just because `cargo` appears
/// somewhere. Network is no longer gated by command name at all (ADR 0022); this only governs plan mode.
fn leading_program(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or("")
}

/// Whether a command introduces a *second* program or a shell expansion that could run one (`;`, `|`,
/// `&&`, background `&`, command substitution, process substitution, newline). When it does, the
/// plan-mode allow-list must not treat the invocation as auto-run-eligible — the leading program no
/// longer characterizes the whole invocation. `2>&1` / `> file` redirections are deliberately not flagged
/// so `cargo build 2>&1` stays fluid. This is a conservative heuristic backing `plan_check`, not a full
/// shell parser; it errs toward requiring confirmation, and `run_command` is confirmed regardless.
fn introduces_another_command(command: &str) -> bool {
    command.contains(';')
        || command.contains('|')
        || command.contains('`')
        || command.contains("$(")
        || command.contains("<(")
        || command.contains(">(")
        || command.contains('\n')
        || command.contains('\r')
        || has_separator_ampersand(command)
}

/// Whether a `&` acts as a command separator (background `&`, `&&`, or a trailing `&`) rather than as
/// part of an fd redirect (`2>&1`, `>&2`, `&>`). A `&` is a separator unless the next byte is an ASCII
/// digit (an fd) or `>` (the `&>`/`>&` forms); a trailing `&` (no next byte) is always a separator.
/// Catches the `&`-without-space form `cargo build &curl http://evil` the old `"& "`/`ends_with('&')`
/// checks missed, which let a backgrounded second program inherit the leading program's plan-mode
/// auto-run eligibility (SEC-02).
fn has_separator_ampersand(command: &str) -> bool {
    let bytes = command.as_bytes();
    bytes.iter().enumerate().any(|(i, &b)| {
        b == b'&'
            && match bytes.get(i + 1) {
                None => true,
                Some(next) => !next.is_ascii_digit() && *next != b'>',
            }
    })
}

/// Best-effort scan of a `run_command` string for a token naming a sensitive file or a credential
/// directory, so the confirmation can warn before the user approves. Returns the first offending token.
///
/// **Not a security control.** It only whitespace-tokenizes, so trivial obfuscation (`c''at $E""NV`,
/// base64, indirect reads, a heredoc) evades it — the OS confinement layer (ADR 0009) is the real
/// boundary, and this must never be sold as a guarantee (security theater is a defect). It only makes an
/// already-confirmed action scarier; it never allows nor denies on its own (the default stays decline).
fn references_sensitive_path(command: &str, sandbox: &dyn Sandbox) -> Option<String> {
    for token in command.split_whitespace() {
        let path = Path::new(token);
        let sensitive_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| sandbox.is_sensitive_name(name));
        if sensitive_name || sandbox.secret_dir_component(path).is_some() {
            return Some(token.to_string());
        }
    }
    None
}

/// The shell-invocation string shown in the confirmation prompt. Single-sourced so `command_line` and
/// `confirmation` render it identically from one parse instead of re-deriving (and re-parsing) it.
fn shell_display(command: &str) -> String {
    if cfg!(windows) {
        format!("$ cmd /C \"{command}\"")
    } else {
        format!("$ sh -c '{command}'")
    }
}

/// Bounds for a `run_command` timeout, so the model cannot pin a process slot for hours nor request a
/// sub-second deadline that kills every command. The applied value is clamped into `[1s, 10min]`.
const RUN_COMMAND_MIN_TIMEOUT_MS: u64 = 1_000;
const RUN_COMMAND_MAX_TIMEOUT_MS: u64 = 600_000;

fn effective_timeout_ms(requested: u64) -> u64 {
    requested.clamp(RUN_COMMAND_MIN_TIMEOUT_MS, RUN_COMMAND_MAX_TIMEOUT_MS)
}

pub struct RunCommand {
    plan_allow: Arc<[Regex]>,
    require_confinement: bool,
}

impl RunCommand {
    pub fn new(plan_allow: Arc<[Regex]>, require_confinement: bool) -> Self {
        Self {
            plan_allow,
            require_confinement,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn schema(&self) -> Value {
        // The output cap and default timeout read their single sources (`EXEC_MAX_BYTES`,
        // `RUN_COMMAND_DEFAULT_TIMEOUT_MS`), so the schema the model sees and the prompt cannot drift
        // from what the harness enforces (SEC-06).
        function_schema(
            self.name(),
            &format!(
                "Run a shell command and return its combined stdout/stderr output. The command runs \
                 in the specified working directory (relative to workspace root, or absolute). Output \
                 is truncated at {} KiB. A timeout (default {} ms / {}s) prevents runaway commands.",
                exec::EXEC_MAX_BYTES / 1024,
                RUN_COMMAND_DEFAULT_TIMEOUT_MS,
                RUN_COMMAND_DEFAULT_TIMEOUT_MS / 1000
            ),
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
                        "description": format!(
                            "Timeout in milliseconds, clamped to [{RUN_COMMAND_MIN_TIMEOUT_MS}, \
                             {RUN_COMMAND_MAX_TIMEOUT_MS}]. Defaults to {RUN_COMMAND_DEFAULT_TIMEOUT_MS}."
                        ),
                        "default": RUN_COMMAND_DEFAULT_TIMEOUT_MS,
                        "minimum": RUN_COMMAND_MIN_TIMEOUT_MS,
                        "maximum": RUN_COMMAND_MAX_TIMEOUT_MS
                    }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let args: RunCommandArgs = parse_args(call).ok()?;
        Some(shell_display(&args.command))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        // Parse once and derive both the display string and the sensitive-path scan from it (the
        // confirmation used to parse twice — once here and again inside `command_line`).
        let args: RunCommandArgs = parse_args(call).ok()?;
        let mut action = format!(
            "Executar comando no shell. {}",
            confirm_execute_suffix(&shell_display(&args.command))
        );
        // SEC-05: the cwd is otherwise invisible in the prompt, yet an absolute / out-of-root cwd widens
        // the command's write sandbox to that directory (`execute` folds it into `extra_rw`). Surface a
        // non-default cwd, and loudly warn when it reaches outside the workspace, before the user approves.
        if is_absolute_target(&args.cwd) {
            action = format!(
                "ATENÇÃO: o comando roda em um diretório fora da workspace ('{}'), o que amplia as \
                 escritas permitidas para lá. {action}",
                args.cwd
            );
        } else if args.cwd != "." && !args.cwd.trim().is_empty() {
            action = format!("{action} (cwd: {})", args.cwd);
        }
        // Defense-in-depth UX (see `references_sensitive_path`): when the command text references a
        // sensitive path, prepend a loud warning so the user reviews before approving. Best-effort only
        // — the OS sandbox is the real control — and it never flips the default, which stays decline.
        if let Some(reference) = references_sensitive_path(&args.command, sandbox) {
            action = format!(
                "ATENÇÃO: este comando referencia um caminho sensível ('{reference}'). O sandbox do SO \
                 é a proteção real; revise antes de aprovar. {action}"
            );
        }
        // run_command is the single highest-blast-radius tool (a full shell), so it always
        // default-declines ([s/N]) regardless of the cwd — a stray Enter must never run an arbitrary
        // command. The cwd location says where it runs, not how dangerous it is.
        Some(confirm(action, false))
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
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

        // Network deny-by-default (ADR 0022): the sandbox's base stance passes straight through, with
        // no per-command widening by leading-program name — a session-wide opt-in
        // (`KIRI_SANDBOX_NETWORK=allow`) is the only way to grant `run_command` network access.
        let network = sandbox.network();
        let result = match exec::run_shell(
            &args.command,
            Some(&cwd),
            Duration::from_millis(effective_timeout_ms(args.timeout_ms)),
            sandbox.confiner(),
            &sandbox.command_policy(network, &[], &[&cwd]),
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

        let content = exec::capped_combined_marking_stderr(&result);
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

    fn plan_check(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let args: RunCommandArgs = parse_args(call).ok()?;
        // Allow-list semantics (default deny): run only when the leading program is explicitly allowed
        // AND the command does not chain a second program — so an allowed prefix can never smuggle a
        // mutating command behind it (`cargo test && rm -rf x`). Reuses the same leading-program /
        // chaining heuristics the network gate relies on.
        let program = leading_program(&args.command);
        let allowed = !program.is_empty()
            && !introduces_another_command(&args.command)
            && self.plan_allow.iter().any(|p| p.is_match(program));
        if allowed {
            None
        } else {
            Some(format!(
                "blocked in plan mode: '{program}' is not in the plan-mode allow-list (read-only \
                 investigation and build/test commands only). Run it outside plan mode."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::fs::default_fs_tools;
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::sandbox::NetworkPolicy;
    use crate::shared::kernel::tool_call::FunctionCall;
    use regex::Regex;
    use serde_json::json;
    use std::fs;
    use std::sync::Arc;

    use tempfile::TempDir;

    fn sandbox(dir: &TempDir) -> FsSandbox {
        FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap()
    }

    fn guarded_sandbox(dir: &TempDir) -> FsSandbox {
        FsSandbox::new(
            dir.path(),
            SensitiveMatcher::new(&[".env", "id_rsa", "*.pem"]).unwrap(),
        )
        .unwrap()
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
        ToolRegistry::new(default_fs_tools(Arc::from(Vec::<Regex>::new()), false))
    }

    use crate::modules::tools::application::registry::ToolRegistry;

    fn bare_run_command() -> RunCommand {
        RunCommand::new(Arc::from(Vec::<Regex>::new()), false)
    }

    fn run_command_with_plan_allow(allow: &[&str]) -> RunCommand {
        let regexes: Vec<Regex> = allow.iter().map(|p| Regex::new(p).unwrap()).collect();
        RunCommand::new(Arc::from(regexes), false)
    }

    #[test]
    fn plan_check_allows_listed_programs_and_blocks_the_rest() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        let rc = run_command_with_plan_allow(&[r"\bcargo\b", r"\brg\b"]);
        // An allow-listed leading program (and a benign 2>&1 redirect) is permitted.
        assert!(
            rc.plan_check(&sb, &call("run_command", json!({"command": "cargo test"})))
                .is_none()
        );
        assert!(
            rc.plan_check(
                &sb,
                &call("run_command", json!({"command": "rg foo src 2>&1"}))
            )
            .is_none()
        );
        // An unlisted program is blocked (default deny).
        assert!(
            rc.plan_check(&sb, &call("run_command", json!({"command": "rm -rf x"})))
                .is_some()
        );
        // Chaining a second program behind an allowed one is blocked outright.
        assert!(
            rc.plan_check(
                &sb,
                &call("run_command", json!({"command": "cargo test && rm -rf x"}))
            )
            .is_some()
        );
        assert!(
            rc.plan_check(
                &sb,
                &call(
                    "run_command",
                    json!({"command": "cargo metadata; curl http://evil"})
                )
            )
            .is_some()
        );
    }

    #[test]
    fn plan_check_blocks_when_allow_list_is_empty() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        let rc = run_command_with_plan_allow(&[]);
        assert!(
            rc.plan_check(&sb, &call("run_command", json!({"command": "ls"})))
                .is_some(),
            "an empty allow-list must deny everything"
        );
    }

    #[test]
    fn network_is_never_widened_by_command_name() {
        // ADR 0022: no command name (allow-listed or not) may widen network on its own — the sandbox's
        // base stance is the only source of truth. Locks the removal of the former dev-command
        // allow-list (issue #5).
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir); // deny-network base
        let rc = bare_run_command();
        for command in ["cargo build", "npm install", "echo hi", "curl http://evil"] {
            let confirmation = rc
                .confirmation(&sb, &call("run_command", json!({ "command": command })))
                .unwrap();
            assert!(
                !confirmation.prompt.contains("acesso à rede"),
                "no command should ever claim a network grant: {}",
                confirmation.prompt
            );
        }
    }

    #[test]
    fn run_command_defaults_to_decline() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        let rc = bare_run_command();
        let confirmation = rc
            .confirmation(&sb, &call("run_command", json!({"command": "echo hi"})))
            .unwrap();
        assert!(
            !confirmation.default_accept,
            "run_command must always default-decline: {}",
            confirmation.prompt
        );
        // Even with a workspace-relative cwd, the default stays decline.
        let confirmation = rc
            .confirmation(
                &sb,
                &call("run_command", json!({"command": "echo hi", "cwd": "."})),
            )
            .unwrap();
        assert!(!confirmation.default_accept);
    }

    #[test]
    fn confirmation_warns_on_out_of_root_cwd() {
        // SEC-05: an absolute / out-of-root cwd widens the write sandbox to that dir — it must be visible
        // and flagged in the prompt (it used to be invisible: only the command string was shown).
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        let rc = bare_run_command();
        let confirmation = rc
            .confirmation(
                &sb,
                &call("run_command", json!({"command": "echo hi", "cwd": "/tmp"})),
            )
            .unwrap();
        assert!(
            confirmation.prompt.contains("fora da workspace")
                && confirmation.prompt.contains("/tmp"),
            "an out-of-root cwd must be surfaced in the prompt: {}",
            confirmation.prompt
        );
        assert!(
            !confirmation.default_accept,
            "run_command always default-declines, cwd warning notwithstanding"
        );
    }

    #[test]
    fn confirmation_shows_a_non_default_relative_cwd() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        let rc = bare_run_command();
        let confirmation = rc
            .confirmation(
                &sb,
                &call("run_command", json!({"command": "echo hi", "cwd": "sub"})),
            )
            .unwrap();
        assert!(
            confirmation.prompt.contains("cwd: sub"),
            "a non-default relative cwd should be shown: {}",
            confirmation.prompt
        );
    }

    #[test]
    fn references_sensitive_path_flags_env_and_secret_dir() {
        let dir = TempDir::new().unwrap();
        let sb = guarded_sandbox(&dir);
        // A sensitive *name* by the file-name guard.
        assert_eq!(
            references_sensitive_path("cat .env", &sb).as_deref(),
            Some(".env")
        );
        // A non-sensitive name inside a credential *dir* — caught by the secret-dir component check.
        assert!(references_sensitive_path("cat ~/.aws/credentials", &sb).is_some());
        // A benign build command flags nothing.
        assert_eq!(references_sensitive_path("cargo build", &sb), None);
    }

    #[test]
    fn confirmation_warns_on_sensitive_reference() {
        let dir = TempDir::new().unwrap();
        let sb = guarded_sandbox(&dir);
        let rc = bare_run_command();
        let confirmation = rc
            .confirmation(&sb, &call("run_command", json!({"command": "cat .env"})))
            .unwrap();
        assert!(
            confirmation.prompt.contains("sensível"),
            "expected the sensitive-path warning in the prompt: {}",
            confirmation.prompt
        );
        // The warning must escalate scrutiny, never relax the default — it stays decline.
        assert!(
            !confirmation.default_accept,
            "the sensitive-path warning must not flip the default to accept"
        );
    }

    #[test]
    fn effective_timeout_ms_clamps_into_range() {
        assert_eq!(effective_timeout_ms(0), 1_000);
        assert_eq!(effective_timeout_ms(500), 1_000);
        assert_eq!(effective_timeout_ms(30_000), 30_000);
        assert_eq!(effective_timeout_ms(u64::MAX), RUN_COMMAND_MAX_TIMEOUT_MS);
    }

    #[test]
    fn schema_default_timeout_equals_the_const() {
        let rc = bare_run_command();
        let schema = rc.schema();
        let default = &schema["function"]["parameters"]["properties"]["timeout_ms"]["default"];
        assert_eq!(default.as_u64(), Some(RUN_COMMAND_DEFAULT_TIMEOUT_MS));
    }

    #[test]
    fn schema_advertises_the_enforced_timeout_bounds() {
        // The advertised minimum/maximum must equal the consts the `effective_timeout_ms` clamp
        // enforces, so the model never reads a range the harness silently overrides.
        let rc = bare_run_command();
        let schema = rc.schema();
        let timeout = &schema["function"]["parameters"]["properties"]["timeout_ms"];
        assert_eq!(
            timeout["minimum"].as_u64(),
            Some(RUN_COMMAND_MIN_TIMEOUT_MS)
        );
        assert_eq!(
            timeout["maximum"].as_u64(),
            Some(RUN_COMMAND_MAX_TIMEOUT_MS)
        );
    }

    #[test]
    fn schema_description_reflects_the_limit_constants() {
        let rc = bare_run_command();
        let schema = rc.schema();
        let description = schema["function"]["description"].as_str().unwrap();
        assert!(
            description.contains(&format!("{} KiB", exec::EXEC_MAX_BYTES / 1024)),
            "the description must render the EXEC_MAX_BYTES-derived KiB: {description}"
        );
        assert!(
            description.contains(&format!("{}s", RUN_COMMAND_DEFAULT_TIMEOUT_MS / 1000)),
            "the description must render the RUN_COMMAND_DEFAULT_TIMEOUT_MS-derived seconds: {description}"
        );
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

        // Generate enough output to exceed exec::EXEC_MAX_BYTES. The shell loop syntax differs
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
    fn confined_sandbox(dir: &TempDir) -> FsSandbox {
        use crate::modules::tools::infrastructure::confine::macos::MacosSeatbelt;
        FsSandbox::with_confinement(
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
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let probe = format!("{home}/kiri-sbx-must-not-exist-{}", std::process::id());
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

    // Same end-to-end proof as the macOS block above, through the Linux bwrap adapter.
    #[cfg(target_os = "linux")]
    fn confined_sandbox(dir: &TempDir) -> FsSandbox {
        use crate::modules::tools::infrastructure::confine::linux::BwrapSandbox;
        FsSandbox::with_confinement(
            dir.path(),
            SensitiveMatcher::empty(),
            Arc::new(BwrapSandbox),
            NetworkPolicy::Deny,
            Arc::from(Vec::new()),
            Arc::from(Vec::new()),
        )
        .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn confined_run_command_cannot_write_outside_root() {
        use crate::modules::tools::infrastructure::confine::linux::BwrapSandbox;
        if BwrapSandbox::detect().is_none() {
            return; // bwrap unavailable/non-functional on this host
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn confined_run_command_still_works_inside_root() {
        use crate::modules::tools::infrastructure::confine::linux::BwrapSandbox;
        if BwrapSandbox::detect().is_none() {
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn confined_run_command_cannot_read_credential_dir() {
        use crate::modules::tools::infrastructure::confine::linux::BwrapSandbox;
        if BwrapSandbox::detect().is_none() {
            return;
        }
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let ssh_dir = std::path::Path::new(&home).join(".ssh");
        if !ssh_dir.is_dir() {
            return; // nothing to probe reading on this host
        }
        let dir = TempDir::new().unwrap();
        let sb = confined_sandbox(&dir);
        let reg = registry();
        let outcome = reg
            .execute(
                &sb,
                &call(
                    "run_command",
                    json!({ "command": format!("ls {}", ssh_dir.display()) }),
                ),
            )
            .await;
        match outcome {
            ToolOutcome::Ok(text) => assert!(
                !text.contains("exit code 0"),
                "listing a shadowed credential dir must not succeed: {text}"
            ),
            ToolOutcome::Error(_) => {}
            ToolOutcome::Declined => panic!("unexpected Declined"),
        }
    }
}
