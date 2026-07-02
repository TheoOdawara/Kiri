use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, function_schema, simple_command,
    simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
use crate::modules::tools::infrastructure::support::{READ_FILE_MAX_BYTES, read_capped};
use crate::shared::kernel::tool_call::ToolCall;

pub struct ReadFile;

#[async_trait::async_trait(?Send)]
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

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("cat {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation("Ler o arquivo", self.command_line(sandbox, call), &a.path)
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: PathArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let path = match sandbox.resolve_existing(&args.path) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        // Bound the read at `READ_FILE_MAX_BYTES + 1` (the `+1` lets the truncation check below tell
        // "exactly at the cap" from "more data follows" without re-stating the file).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::tool_call::FunctionCall;
    use std::fs;
    use tempfile::TempDir;

    fn call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn read_file_returns_the_content() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("f.txt"), "héllo").unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ReadFile.execute(&sb, &call(json!({"path": "f.txt"}))).await;
        match outcome {
            ToolOutcome::Ok(text) => assert_eq!(text, "héllo"),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_file_truncates_at_the_byte_cap() {
        let dir = TempDir::new().unwrap();
        let big = "a".repeat(READ_FILE_MAX_BYTES + 100);
        fs::write(dir.path().join("big.txt"), &big).unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ReadFile
            .execute(&sb, &call(json!({"path": "big.txt"})))
            .await;
        match outcome {
            ToolOutcome::Ok(text) => assert!(
                text.contains("truncated at"),
                "expected a truncation marker: {text}"
            ),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_file_errors_on_a_missing_file() {
        let dir = TempDir::new().unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ReadFile
            .execute(&sb, &call(json!({"path": "nope.txt"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }
}
