#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

#[cfg(unix)]
use crate::modules::tools::application::command_sandbox::NetworkPolicy;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema, simple_command,
};
use crate::modules::tools::infrastructure::args::{ListArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::sandbox::{Sandbox, default_accept_for};
use crate::shared::kernel::tool_call::ToolCall;

pub struct ListDir;

#[async_trait::async_trait(?Send)]
impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "List the entries of a directory (one level). Defaults to the workspace root. Directories \
             are suffixed with '/'.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "path": { "type": "string", "description": "Directory path relative to the workspace root. Defaults to '.'." } }
            }),
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &ListArgs| format!("ls {}", a.path))
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        let a: ListArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(confirm(
            format!("Listar o diretório. Aprova executar: {cmd}?"),
            default_accept_for(&a.path),
        ))
    }

    async fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: ListArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        // `resolve_existing` refuses a path inside a credential directory (`.ssh`/…), so listing one is
        // rejected there — no separate secret-dir check needed here.
        let dir = match sandbox.resolve_existing(&args.path) {
            Ok(dir) => dir,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        // `ls -1A -p` lists one entry per line, excludes `.`/`..`, and marks directories with `/`.
        // `QUOTING_STYLE=literal` stops GNU `ls` from quoting unusual names; the lines are re-sorted in
        // Rust so the order is byte-lexicographic and locale-independent (matching the native version).
        #[cfg(unix)]
        let mut names: Vec<String> = {
            let cwd = sandbox.exec_cwd_for(&dir);
            let result = match exec::run_argv(
                &[
                    OsStr::new("ls"),
                    OsStr::new("-1A"),
                    OsStr::new("-p"),
                    dir.as_os_str(),
                ],
                Some(&cwd),
                None,
                &[("QUOTING_STYLE", OsStr::new("literal"))],
                exec::DEFAULT_TIMEOUT,
                sandbox.confiner(),
                &sandbox.command_policy(NetworkPolicy::Deny, &[&cwd]),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    return ToolOutcome::Error(format!(
                        "cannot list {}: {}",
                        args.path,
                        error.message()
                    ));
                }
            };
            if !result.succeeded() {
                return ToolOutcome::Error(format!(
                    "cannot list {}: {}",
                    args.path,
                    result.stderr_text()
                ));
            }
            String::from_utf8_lossy(&result.stdout)
                .lines()
                .map(|line| line.to_string())
                .collect()
        };

        #[cfg(windows)]
        let mut names: Vec<String> = {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) => {
                    return ToolOutcome::Error(format!("cannot list {}: {error}", args.path));
                }
            };
            let mut names = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
                names.push(if is_dir { format!("{name}/") } else { name });
            }
            names
        };

        names.sort();
        if names.is_empty() {
            ToolOutcome::Ok("(empty)".to_string())
        } else {
            ToolOutcome::Ok(names.join("\n"))
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::sandbox::Sandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};
    use std::path::PathBuf;

    fn sandbox() -> Sandbox {
        Sandbox::new(PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
    }

    fn call(args: &str) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "list_dir".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[test]
    fn confirmation_shows_the_real_command() {
        let s = sandbox();
        let c = ListDir
            .confirmation(&s, &call(r#"{"path":"src"}"#))
            .unwrap();
        assert!(
            c.prompt.contains("ls src"),
            "expected the real command in the prompt: {}",
            c.prompt
        );
    }
}
