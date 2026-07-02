use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_command, simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
use crate::modules::tools::infrastructure::support::stat_guard;
use crate::shared::kernel::tool_call::ToolCall;

pub struct DeleteDir;

#[async_trait::async_trait(?Send)]
impl Tool for DeleteDir {
    fn name(&self) -> &'static str {
        "delete_dir"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Delete a directory and all of its contents, recursively. Requires confirmation. The path \
             is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Directory path to delete, relative to the workspace root." } }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("rm -rf {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation(
            "Excluir recursivamente o diretório e todo o seu conteúdo",
            self.command_line(sandbox, call),
            &a.path,
        )
    }

    fn confirm_in_auto(&self) -> bool {
        true
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
        if path == sandbox.root() {
            return ToolOutcome::Error("refusing to delete the workspace root".to_string());
        }
        if let Err(out) = stat_guard(&path, &args.path, |metadata| {
            (!metadata.is_dir())
                .then(|| format!("{} is not a directory; use delete_file", args.path))
        })
        .await
        {
            return out;
        }

        match std::fs::remove_dir_all(&path) {
            Ok(()) => ToolOutcome::Ok(format!("deleted directory {}", args.path)),
            Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
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
                name: "delete_dir".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn delete_dir_removes_the_tree_recursively() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("sub/nested")).unwrap();
        fs::write(dir.path().join("sub/nested/f.txt"), b"x").unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = DeleteDir.execute(&sb, &call(json!({"path": "sub"}))).await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(!dir.path().join("sub").exists());
    }

    #[tokio::test]
    async fn delete_dir_refuses_the_workspace_root() {
        let dir = TempDir::new().unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = DeleteDir.execute(&sb, &call(json!({"path": "."}))).await;
        match outcome {
            ToolOutcome::Error(msg) => assert!(msg.contains("workspace root"), "got: {msg}"),
            other => panic!("expected refusal, got {other:?}"),
        }
        assert!(dir.path().is_dir());
    }
}
