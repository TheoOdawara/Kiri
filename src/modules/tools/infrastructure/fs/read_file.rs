use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, is_absolute_target};
use crate::modules::tools::infrastructure::support::{READ_FILE_MAX_BYTES, read_capped};
use crate::shared::kernel::tool_call::ToolCall;

pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Read a UTF-8 text file and return its contents. Paths are relative to the active workspace \
             root; an absolute path or '~/…' may reach outside it (the user confirms each call). '..' \
             in a relative path is rejected.",
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
        Some(confirm(format!("Ler '{}'?", a.path), default_accept))
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
        let bytes = match read_capped(&path, READ_FILE_MAX_BYTES + 1) {
            Ok(bytes) => bytes,
            Err(error) => return ToolOutcome::Error(format!("cannot read {}: {error}", args.path)),
        };
        if bytes.len() > READ_FILE_MAX_BYTES {
            let head = String::from_utf8_lossy(&bytes[..READ_FILE_MAX_BYTES]);
            ToolOutcome::Ok(format!(
                "{head}\n… (truncated at {READ_FILE_MAX_BYTES} bytes)"
            ))
        } else {
            ToolOutcome::Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}
