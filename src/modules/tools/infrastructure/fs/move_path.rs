use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{MoveArgs, parse, parse_args};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, is_absolute_target};
use crate::modules::tools::infrastructure::support::{ensure_parent_dirs, missing_dirs_label};
use crate::shared::kernel::tool_call::ToolCall;

pub struct MovePath;

impl Tool for MovePath {
    fn name(&self) -> &'static str {
        "move_path"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Move or rename a file or directory. Creates missing parent directories of the destination \
             (with user confirmation) and overwrites an existing destination (with confirmation). Both \
             paths are relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["source", "destination"],
                "properties": {
                    "source": { "type": "string", "description": "Existing file or directory to move, relative to the workspace root." },
                    "destination": { "type": "string", "description": "New path, relative to the workspace root." }
                }
            }),
        )
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: MoveArgs = parse(call.function.arguments.as_str()).ok()?;
        let action = match sandbox.resolve_create(&a.destination) {
            Ok(r) if !r.missing_dirs.is_empty() => format!(
                "Criar diretório(s) '{}' e mover '{}' → '{}'?",
                missing_dirs_label(&r, sandbox),
                a.source,
                a.destination
            ),
            Ok(r) if r.target.exists() => {
                format!(
                    "Sobrescrever '{}' movendo de '{}'?",
                    a.destination, a.source
                )
            }
            _ => format!("Mover '{}' → '{}'?", a.source, a.destination),
        };
        let default_accept = !is_absolute_target(&a.destination);
        Some(confirm(action, default_accept))
    }

    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: MoveArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let source = match sandbox.resolve_existing(&args.source) {
            Ok(source) => source,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if source == sandbox.root() {
            return ToolOutcome::Error("refusing to move the workspace root".to_string());
        }
        let resolution = match sandbox.resolve_create(&args.destination) {
            Ok(resolution) => resolution,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if let Err(out) = ensure_parent_dirs(&resolution, &args.destination) {
            return out;
        }
        match fs::rename(&source, &resolution.target) {
            Ok(()) => ToolOutcome::Ok(format!("moved {} to {}", args.source, args.destination)),
            Err(error) => ToolOutcome::Error(format!(
                "cannot move {} to {}: {error}",
                args.source, args.destination
            )),
        }
    }
}
