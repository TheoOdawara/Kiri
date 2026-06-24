use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, function_schema, simple_confirm,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse_args};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
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
                "properties": { "path": { "type": "string", "description": PATH_DESC } }
            }),
        )
    }

    fn confirmation(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        simple_confirm(
            call,
            |a: &PathArgs| format!("Ler '{}'?", a.path),
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

    fn is_read_only(&self) -> bool {
        true
    }
}
