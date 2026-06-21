use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, is_absolute_target};
use crate::shared::kernel::tool_call::ToolCall;

pub struct DeleteDir;

impl Tool for DeleteDir {
    fn name(&self) -> &'static str {
        "delete_dir"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Delete a directory and all of its contents, recursively. Requires confirmation. The path \
             is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Directory path to delete, relative to the workspace root." } }
            }),
        )
    }

    fn confirmation(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        let default_accept = !is_absolute_target(&a.path);
        Some(confirm(
            format!(
                "Excluir RECURSIVAMENTE o diretório '{}' e TODO o seu conteúdo?",
                a.path
            ),
            default_accept,
        ))
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
        if path == sandbox.root() {
            return ToolOutcome::Error("refusing to delete the workspace root".to_string());
        }
        match fs::metadata(&path) {
            Ok(metadata) if !metadata.is_dir() => {
                return ToolOutcome::Error(format!(
                    "{} is not a directory; use delete_file",
                    args.path
                ));
            }
            Ok(_) => {}
            Err(error) => return ToolOutcome::Error(format!("cannot stat {}: {error}", args.path)),
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => ToolOutcome::Ok(format!("deleted directory {}", args.path)),
            Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
        }
    }
}
