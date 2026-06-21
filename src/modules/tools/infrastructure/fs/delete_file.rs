use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, is_absolute_target};
use crate::shared::kernel::tool_call::ToolCall;

pub struct DeleteFile;

impl Tool for DeleteFile {
    fn name(&self) -> &'static str {
        "delete_file"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Delete a file (not a directory). The path is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Path relative to the active workspace root, or an absolute / ~ path to reach outside it." } }
            }),
        )
    }

    fn confirmation(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        let default_accept = !is_absolute_target(&a.path);
        Some(confirm(format!("Excluir '{}'?", a.path), default_accept))
    }

    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args = call.function.arguments.as_str();
        let args: PathArgs = match parse(args) {
            Ok(args) => args,
            Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
        };
        let path = match sandbox.resolve_existing(&args.path) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                return ToolOutcome::Error(format!("{} is a directory; not deleted", args.path));
            }
            Ok(_) => {}
            Err(error) => return ToolOutcome::Error(format!("cannot stat {}: {error}", args.path)),
        }
        match fs::remove_file(&path) {
            Ok(()) => ToolOutcome::Ok(format!("deleted {}", args.path)),
            Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
        }
    }
}
