use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_confirm,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse_args};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tools::infrastructure::support::stat_guard;
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
        simple_confirm(
            call,
            |a: &PathArgs| {
                format!(
                    "Excluir RECURSIVAMENTE o diretório '{}' e TODO o seu conteúdo?",
                    a.path
                )
            },
            |a| a.path.as_str(),
        )
    }

    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: PathArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let path = match sandbox.resolve_existing(&args.path) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if path == sandbox.root() {
            return ToolOutcome::Error("refusing to delete the workspace root".to_string());
        }
        if let Err(out) = stat_guard(&path, &args.path, |metadata| {
            (!metadata.is_dir())
                .then(|| format!("{} is not a directory; use delete_file", args.path))
        }) {
            return out;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => ToolOutcome::Ok(format!("deleted directory {}", args.path)),
            Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
        }
    }
}
