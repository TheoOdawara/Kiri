use serde_json::{Value, json};

use crate::modules::tools::application::path::default_accept_for;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{PathArgs, WriteArgs, parse, parse_args};
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::support::{ensure_parent_dirs, missing_dirs_label};
use crate::shared::kernel::tool_call::ToolCall;

pub struct WriteFile;

#[async_trait::async_trait(?Send)]
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
                    "path": { "type": "string", "description": PATH_DESC },
                    "content": { "type": "string", "description": "Full file content to write." }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("write {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        let cmd = self.command_line(sandbox, call)?;
        let action = match sandbox.resolve_create(&a.path) {
            Ok(r) if !r.missing_dirs.is_empty() => format!(
                "Criar diretório(s) '{}' e gravar o arquivo. Aprova executar: {cmd}?",
                missing_dirs_label(&r, sandbox),
            ),
            Ok(r) if r.target.exists() => {
                format!("Sobrescrever o arquivo. Aprova executar: {cmd}?")
            }
            // Also covers a resolve_create error: the user is still asked (returning None here would
            // skip confirmation), and execute() re-validates the path and surfaces the real error.
            _ => format!("Criar e gravar o arquivo. Aprova executar: {cmd}?"),
        };
        let default_accept = default_accept_for(&a.path);
        Some(confirm(action, default_accept))
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: WriteArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let resolution = match sandbox.resolve_create(&args.path) {
            Ok(resolution) => resolution,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if let Err(out) = ensure_parent_dirs(&resolution, &args.path).await {
            return out;
        }

        match tokio::time::timeout(
            exec::DEFAULT_TIMEOUT,
            tokio::fs::write(&resolution.target, args.content.as_bytes()),
        )
        .await
        {
            Ok(Ok(())) => ToolOutcome::Ok(format!(
                "wrote {} bytes to {}",
                args.content.len(),
                args.path
            )),
            Ok(Err(error)) => ToolOutcome::Error(format!("cannot write {}: {error}", args.path)),
            // `tokio::fs` runs on the blocking pool and can't be cancelled once dispatched: the write
            // may still land after this timeout is reported (issue #53, security-debt).
            Err(_) => ToolOutcome::Error(format!(
                "cannot write {}: timed out (it may still complete in the background)",
                args.path
            )),
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
                name: "write_file".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn write_file_creates_missing_parent_dirs_and_writes_content() {
        let dir = TempDir::new().unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = WriteFile
            .execute(&sb, &call(json!({"path": "a/b/c.txt", "content": "hello"})))
            .await;
        assert!(
            matches!(outcome, ToolOutcome::Ok(_)),
            "expected Ok, got {outcome:?}"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn write_file_overwrites_an_existing_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("f.txt"), b"old").unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = WriteFile
            .execute(&sb, &call(json!({"path": "f.txt", "content": "new"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert_eq!(fs::read_to_string(dir.path().join("f.txt")).unwrap(), "new");
    }
}
