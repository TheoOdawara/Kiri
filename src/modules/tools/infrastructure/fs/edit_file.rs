use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, function_schema, simple_command,
    simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{EditArgs, PathArgs, parse, parse_args};
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::support::{EDIT_FILE_MAX_BYTES, stat_guard};
use crate::shared::kernel::tool_call::ToolCall;

pub struct EditFile;

#[async_trait::async_trait(?Send)]
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

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("edit {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation(
            "Editar o arquivo",
            self.command_line(sandbox, call),
            &a.path,
        )
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
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
        })
        .await
        {
            return out;
        }

        // Native read/find/splice/write on the already-validated, canonicalized path — identical on
        // every platform. Replaces the former `python3` shell-out (absent on a clean macOS, the v1
        // target, which made every edit fail there); old/new are Rust values spliced in memory, so no
        // content is ever interpreted by a shell or an interpreter. Both I/O awaits are bounded by
        // `DEFAULT_TIMEOUT` (the all-I/O-has-a-timeout rule): a wedged/stale mount must fail fast rather
        // than stall the single-threaded TUI runtime, the exact hang class the contract forbids.
        let content =
            match tokio::time::timeout(exec::DEFAULT_TIMEOUT, tokio::fs::read_to_string(&path))
                .await
            {
                Ok(Ok(content)) => content,
                Ok(Err(error)) => {
                    return ToolOutcome::Error(format!(
                        "cannot read {} as text: {error}",
                        args.path
                    ));
                }
                Err(_) => {
                    return ToolOutcome::Error(format!("cannot read {}: timed out", args.path));
                }
            };
        let Some(position) = content.find(&args.old_string) else {
            return ToolOutcome::Error(format!("old_string not found in {}", args.path));
        };
        let mut updated = String::with_capacity(content.len() + args.new_string.len());
        updated.push_str(&content[..position]);
        updated.push_str(&args.new_string);
        updated.push_str(&content[position + args.old_string.len()..]);
        match tokio::time::timeout(
            exec::DEFAULT_TIMEOUT,
            tokio::fs::write(&path, updated.as_bytes()),
        )
        .await
        {
            Ok(Ok(())) => ToolOutcome::Ok(format!("edited {}", args.path)),
            Ok(Err(error)) => ToolOutcome::Error(format!("cannot write {}: {error}", args.path)),
            Err(_) => ToolOutcome::Error(format!("cannot write {}: timed out", args.path)),
        }
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
                name: "edit_file".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    // Regression for the timed `tokio::fs` read/write path (TOOL-06): the splice still produces the
    // same result as the former synchronous `std::fs` calls.
    #[tokio::test]
    async fn edit_file_still_edits() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("f.txt"), b"hello world").unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = EditFile
            .execute(
                &sb,
                &call(json!({"path": "f.txt", "old_string": "world", "new_string": "kiri"})),
            )
            .await;
        assert!(
            matches!(outcome, ToolOutcome::Ok(_)),
            "expected Ok, got {outcome:?}"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "hello kiri"
        );
    }
}
