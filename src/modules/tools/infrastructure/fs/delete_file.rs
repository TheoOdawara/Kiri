use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, function_schema, simple_confirm,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse_args};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tools::infrastructure::support::stat_guard;
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
                "properties": { "path": { "type": "string", "description": PATH_DESC } }
            }),
        )
    }

    fn confirmation(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        simple_confirm(
            call,
            |a: &PathArgs| format!("Excluir o arquivo. Aprova executar: rm {}?", a.path),
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
        if let Err(out) = stat_guard(&path, &args.path, |metadata| {
            metadata
                .is_dir()
                .then(|| format!("{} is a directory; not deleted", args.path))
        }) {
            return out;
        }
        match fs::remove_file(&path) {
            Ok(()) => ToolOutcome::Ok(format!("deleted {}", args.path)),
            Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
        }
    }
}
