use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_command, simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
use crate::modules::tools::infrastructure::exec;
use crate::shared::kernel::tool_call::ToolCall;

pub struct CreateDir;

#[async_trait::async_trait(?Send)]
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

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("mkdir {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation("Criar diretório", self.command_line(sandbox, call), &a.path)
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: PathArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let resolution = match sandbox.resolve_create(&args.path) {
            Ok(resolution) => resolution,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        // Bounded stat: `Path::is_dir` is a blocking syscall that can hang on a wedged mount just
        // like the create below; a stat failure (including "does not exist") falls through to the
        // create attempt, which surfaces the real error if there is one.
        let already_exists = match tokio::time::timeout(
            exec::DEFAULT_TIMEOUT,
            tokio::fs::metadata(&resolution.target),
        )
        .await
        {
            Ok(Ok(metadata)) => metadata.is_dir(),
            Ok(Err(_)) => false,
            Err(_) => return ToolOutcome::Error(format!("cannot stat {}: timed out", args.path)),
        };
        if already_exists {
            return ToolOutcome::Ok(format!("directory already exists: {}", args.path));
        }

        match tokio::time::timeout(
            exec::DEFAULT_TIMEOUT,
            tokio::fs::create_dir_all(&resolution.target),
        )
        .await
        {
            Ok(Ok(())) => ToolOutcome::Ok(format!("created directory {}", args.path)),
            Ok(Err(error)) => ToolOutcome::Error(format!("cannot create {}: {error}", args.path)),
            // `tokio::fs` runs on the blocking pool and can't be cancelled once dispatched: the mkdir
            // may still land after this timeout is reported (issue #53, security-debt).
            Err(_) => ToolOutcome::Error(format!(
                "cannot create {}: timed out (it may still complete in the background)",
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
    use tempfile::TempDir;

    fn call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "create_dir".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn create_dir_makes_missing_intermediate_dirs() {
        let dir = TempDir::new().unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = CreateDir
            .execute(&sb, &call(json!({"path": "a/b/c"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[tokio::test]
    async fn create_dir_is_a_noop_ok_when_already_present() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("existing")).unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = CreateDir
            .execute(&sb, &call(json!({"path": "existing"})))
            .await;
        match outcome {
            ToolOutcome::Ok(msg) => assert!(msg.contains("already exists"), "got: {msg}"),
            other => panic!("expected Ok, got {other:?}"),
        }
    }
}
