use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{ListArgs, parse, parse_args};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, is_absolute_target};
use crate::shared::kernel::tool_call::ToolCall;

pub struct ListDir;

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

    fn confirmation(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: ListArgs = parse(call.function.arguments.as_str()).ok()?;
        let default_accept = !is_absolute_target(&a.path);
        Some(confirm(format!("Listar '{}'?", a.path), default_accept))
    }

    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
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
}
