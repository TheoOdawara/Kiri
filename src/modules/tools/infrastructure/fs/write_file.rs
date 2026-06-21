use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{PathArgs, WriteArgs, parse};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, is_absolute_target};
use crate::modules::tools::infrastructure::support::missing_dirs_label;
use crate::shared::kernel::tool_call::ToolCall;

pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Create a file or overwrite it with the full given content. Creates missing parent \
             directories (with user confirmation). The path is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the active workspace root, or an absolute / ~ path to reach outside it." },
                    "content": { "type": "string", "description": "Full file content to write." }
                }
            }),
        )
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        let action = match sandbox.resolve_create(&a.path) {
            Ok(r) if !r.missing_dirs.is_empty() => format!(
                "Criar diretório(s) '{}' e gravar '{}'?",
                missing_dirs_label(&r, sandbox),
                a.path
            ),
            Ok(r) if r.target.exists() => format!("Sobrescrever '{}'?", a.path),
            _ => format!("Criar e gravar '{}'?", a.path),
        };
        let default_accept = !is_absolute_target(&a.path);
        Some(confirm(action, default_accept))
    }

    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args = call.function.arguments.as_str();
        let args: WriteArgs = match parse(args) {
            Ok(args) => args,
            Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
        };
        let resolution = match sandbox.resolve_create(&args.path) {
            Ok(resolution) => resolution,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if !resolution.missing_dirs.is_empty()
            && let Some(parent) = resolution.target.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            return ToolOutcome::Error(format!(
                "cannot create directories for {}: {error}",
                args.path
            ));
        }
        match fs::write(&resolution.target, args.content.as_bytes()) {
            Ok(()) => ToolOutcome::Ok(format!(
                "wrote {} bytes to {}",
                args.content.len(),
                args.path
            )),
            Err(error) => ToolOutcome::Error(format!("cannot write {}: {error}", args.path)),
        }
    }
}
