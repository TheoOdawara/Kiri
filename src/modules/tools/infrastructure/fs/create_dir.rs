use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_confirm,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse_args};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::kernel::tool_call::ToolCall;

pub struct CreateDir;

impl Tool for CreateDir {
    fn name(&self) -> &'static str {
        "create_dir"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Create a directory, including any missing parent directories. The path is relative to the \
             workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Directory path to create, relative to the workspace root." } }
            }),
        )
    }

    fn confirmation(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        simple_confirm(
            call,
            |a: &PathArgs| format!("Criar diretório '{}'?", a.path),
            |a| a.path.as_str(),
        )
    }

    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: PathArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let resolution = match sandbox.resolve_create(&args.path) {
            Ok(resolution) => resolution,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if resolution.target.is_dir() {
            return ToolOutcome::Ok(format!("directory already exists: {}", args.path));
        }
        match fs::create_dir_all(&resolution.target) {
            Ok(()) => ToolOutcome::Ok(format!("created directory {}", args.path)),
            Err(error) => ToolOutcome::Error(format!("cannot create {}: {error}", args.path)),
        }
    }
}
