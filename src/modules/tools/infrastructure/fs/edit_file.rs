use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, function_schema, simple_confirm,
};
use crate::modules::tools::infrastructure::args::{EditArgs, PathArgs, parse_args};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tools::infrastructure::support::{EDIT_FILE_MAX_BYTES, stat_guard};
use crate::shared::kernel::tool_call::ToolCall;

pub struct EditFile;

impl Tool for EditFile {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Replace the first exact occurrence of old_string with new_string in an existing file.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "old_string", "new_string"],
                "properties": {
                    "path": { "type": "string", "description": PATH_DESC },
                    "old_string": { "type": "string", "description": "Exact text to find (must be unique enough)." },
                    "new_string": { "type": "string", "description": "Replacement text." }
                }
            }),
        )
    }

    fn confirmation(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        simple_confirm(
            call,
            |a: &PathArgs| format!("Editar '{}'?", a.path),
            |a| a.path.as_str(),
        )
    }

    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: EditArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        if args.old_string.is_empty() {
            return ToolOutcome::Error("old_string must not be empty".to_string());
        }
        let path = match sandbox.resolve_existing(&args.path) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if let Err(out) = stat_guard(&path, &args.path, |metadata| {
            (metadata.len() > EDIT_FILE_MAX_BYTES).then(|| {
                format!(
                    "{} is too large to edit (max {EDIT_FILE_MAX_BYTES} bytes)",
                    args.path
                )
            })
        }) {
            return out;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                return ToolOutcome::Error(format!("cannot read {} as text: {error}", args.path));
            }
        };
        let Some(position) = content.find(&args.old_string) else {
            return ToolOutcome::Error(format!("old_string not found in {}", args.path));
        };
        let mut updated = String::with_capacity(content.len() + args.new_string.len());
        updated.push_str(&content[..position]);
        updated.push_str(&args.new_string);
        updated.push_str(&content[position + args.old_string.len()..]);
        match fs::write(&path, updated.as_bytes()) {
            Ok(()) => ToolOutcome::Ok(format!("edited {}", args.path)),
            Err(error) => ToolOutcome::Error(format!("cannot write {}: {error}", args.path)),
        }
    }
}
