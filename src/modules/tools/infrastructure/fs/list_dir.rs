use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_command, simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{ListArgs, parse, parse_args};
use crate::shared::kernel::tool_call::ToolCall;

pub struct ListDir;

#[async_trait::async_trait(?Send)]
impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "List the entries of a directory (one level). Defaults to the workspace root. Directories \
             are suffixed with '/'.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "path": { "type": "string", "description": "Directory path relative to the workspace root. Defaults to '.'." } }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &ListArgs| format!("ls {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: ListArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation(
            "Listar o diretório",
            self.command_line(sandbox, call),
            &a.path,
        )
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: ListArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        // `resolve_existing` refuses a path inside a credential directory (`.ssh`/…), so listing one is
        // rejected there — no separate secret-dir check needed here.
        let dir = match sandbox.resolve_existing(&args.path) {
            Ok(dir) => dir,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        // One entry per line, excluding `.`/`..`, directories suffixed with `/`. `file_type()` reads the
        // entry's own type without following a symlink, so a symlink (including a symlink-to-directory)
        // is never marked as a directory — the lines are sorted below so the order is
        // byte-lexicographic and locale-independent.
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                return ToolOutcome::Error(format!("cannot list {}: {error}", args.path));
            }
        };
        let mut names: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            names.push(if is_dir { format!("{name}/") } else { name });
        }

        names.sort();
        if names.is_empty() {
            ToolOutcome::Ok("(empty)".to_string())
        } else {
            ToolOutcome::Ok(names.join("\n"))
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
    use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};
    use std::path::PathBuf;

    fn sandbox() -> FsSandbox {
        FsSandbox::new(PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
    }

    fn call(args: &str) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "list_dir".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[test]
    fn confirmation_shows_the_real_command() {
        let s = sandbox();
        let c = ListDir
            .confirmation(&s, &call(r#"{"path":"src"}"#))
            .unwrap();
        assert!(
            c.prompt.contains("ls src"),
            "expected the real command in the prompt: {}",
            c.prompt
        );
    }

    #[tokio::test]
    async fn list_dir_sorts_entries_and_suffixes_directories() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("b.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("a_dir")).unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ListDir.execute(&sb, &call("{}")).await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert_eq!(text, "a_dir/\nb.txt");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_dir_reports_empty_for_an_empty_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = ListDir.execute(&sb, &call("{}")).await;
        assert!(matches!(outcome, ToolOutcome::Ok(text) if text == "(empty)"));
    }
}
