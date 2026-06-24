use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema, simple_command,
};
use crate::modules::tools::infrastructure::args::{ListArgs, parse, parse_args};
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
        let dir = match sandbox.resolve_existing(&args.path) {
            Ok(dir) => dir,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => return ToolOutcome::Error(format!("cannot list {}: {error}", args.path)),
        };
        let mut names: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            names.push(if is_dir { format!("{name}/") } else { name });
        }
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
