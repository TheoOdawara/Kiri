use crate::modules::agent::application::approval_policy::ApprovalMode;
use crate::modules::tools::application::tool::{Confirmation, Tool, ToolOutcome};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::kernel::tool_call::ToolCall;

/// Holds the registered tools and dispatches by name. Replaces the central `tool_definitions`/
/// `execute`/`confirmation_prompt` match: a tool advertises, confirms, and runs itself.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self { tools }
    }

    /// The schema array advertised to the provider (replaces `tool_definitions()`).
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools.iter().map(|tool| tool.schema()).collect()
    }

    /// The schema array advertised for `mode`. In plan mode only plannable tools are offered, so the
    /// model can investigate (read files, run dev servers, search logs) but not mutate the project
    /// directly — `run_command` is plannable but its plan-mode blacklist handles destructive shell
    /// commands at execution time.
    pub fn schemas_for(&self, mode: ApprovalMode) -> Vec<serde_json::Value> {
        if mode == ApprovalMode::Plan {
            self.tools
                .iter()
                .filter(|tool| tool.is_plannable())
                .map(|tool| tool.schema())
                .collect()
        } else {
            self.schemas()
        }
    }

    /// Whether a named tool exists and mutates the filesystem. Currently unused in the engine path
    /// (plan mode gates on `is_plannable` instead) but kept as a classification test assertion and
    /// for future use (e.g. destructive-tool warnings in non-plan modes).
    #[allow(dead_code)]
    pub fn is_destructive(&self, name: &str) -> bool {
        self.find(name).is_some_and(|tool| !tool.is_read_only())
    }

    /// Whether a named tool is advertised in plan mode. Plannable tools either never mutate
    /// (`is_read_only`) or opt in explicitly (`is_plannable` override, e.g. `run_command`).
    pub fn is_plannable(&self, name: &str) -> bool {
        self.find(name).is_some_and(|tool| tool.is_plannable())
    }

    /// Whether a named tool must be confirmed even in auto mode (irreversible / high blast radius).
    /// An unknown tool is not gated — `execute` reports the unknown-tool error instead.
    pub fn confirm_in_auto(&self, name: &str) -> bool {
        self.find(name).is_some_and(|tool| tool.confirm_in_auto())
    }

    /// In plan mode, ask the named tool whether the call should be blocked. Returns
    /// `Some(reason)` if the tool refuses the call, `None` if it's allowed.
    pub fn plan_check(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        self.find(&call.function.name)?.plan_check(sandbox, call)
    }

    fn find(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .map(|tool| tool.as_ref())
            .find(|tool| tool.name() == name)
    }

    pub fn confirm(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        self.find(&call.function.name)?.confirmation(sandbox, call)
    }

    /// The bare command label for a call, for on-screen display. `None` for an unknown tool or
    /// unparseable args (the caller falls back to the tool name).
    pub fn command_line(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        self.find(&call.function.name)?.command_line(sandbox, call)
    }

    pub async fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        match self.find(&call.function.name) {
            Some(tool) => tool.execute(sandbox, call).await,
            None => ToolOutcome::Error(format!("unknown tool '{}'", call.function.name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::fs::default_fs_tools;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::modules::tools::infrastructure::support::READ_FILE_MAX_BYTES;
    use crate::shared::kernel::tool_call::FunctionCall;
    use regex::Regex;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            let pid = std::process::id();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            path.push(format!("t-cli-tools-{tag}-{pid}-{n}"));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn registry() -> ToolRegistry {
        ToolRegistry::new(default_fs_tools(Arc::from(Vec::<Regex>::new())))
    }

    fn sandbox(dir: &TempDir) -> Sandbox {
        Sandbox::new(&dir.path, SensitiveMatcher::empty()).unwrap()
    }

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn schemas_expose_all_tools_in_order() {
        let names: Vec<String> = registry()
            .schemas()
            .iter()
            .map(|schema| schema["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "move_path",
                "list_dir",
                "create_dir",
                "delete_dir",
                "search",
                "run_command"
            ]
        );
    }

    #[tokio::test]
    async fn plan_mode_schemas_expose_only_plannable_tools() {
        let names: Vec<String> = registry()
            .schemas_for(ApprovalMode::Plan)
            .iter()
            .map(|schema| schema["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["read_file", "list_dir", "search", "run_command"]
        );
    }

    #[tokio::test]
    async fn is_destructive_classifies_tools() {
        let r = registry();
        assert!(r.is_destructive("write_file"));
        assert!(r.is_destructive("delete_dir"));
        assert!(r.is_destructive("move_path"));
        assert!(r.is_destructive("run_command"));
        assert!(!r.is_destructive("read_file"));
        assert!(!r.is_destructive("search"));
        // An unknown tool is not destructive; execute reports the unknown-tool error instead.
        assert!(!r.is_destructive("nope"));
    }

    #[tokio::test]
    async fn command_line_returns_the_bare_command_per_tool() {
        let dir = TempDir::new("cmdline");
        let sb = sandbox(&dir);
        let reg = registry();
        assert_eq!(
            reg.command_line(&sb, &call("read_file", json!({"path": "a.txt"}))),
            Some("cat a.txt".to_string())
        );
        assert_eq!(
            reg.command_line(
                &sb,
                &call(
                    "edit_file",
                    json!({"path": "a.txt", "old_string": "x", "new_string": "y"})
                )
            ),
            Some("edit a.txt".to_string())
        );
        assert_eq!(
            reg.command_line(
                &sb,
                &call("write_file", json!({"path": "a.txt", "content": "x"}))
            ),
            Some("write a.txt".to_string())
        );
        assert_eq!(
            reg.command_line(&sb, &call("search", json!({"query": "q", "path": "."}))),
            Some("rg 'q' .".to_string())
        );
        assert_eq!(
            reg.command_line(
                &sb,
                &call("move_path", json!({"source": "a", "destination": "b"}))
            ),
            Some("mv a b".to_string())
        );
        // run_command shows the shell wrapper — platform-specific.
        #[cfg(unix)]
        assert_eq!(
            reg.command_line(&sb, &call("run_command", json!({"command": "ls"}))),
            Some("$ sh -c 'ls'".to_string())
        );
        #[cfg(windows)]
        assert_eq!(
            reg.command_line(&sb, &call("run_command", json!({"command": "ls"}))),
            Some("$ cmd /C \"ls\"".to_string())
        );
    }

    #[tokio::test]
    async fn command_line_is_none_for_unknown_tool_or_bad_args() {
        let dir = TempDir::new("cmdline-none");
        let sb = sandbox(&dir);
        let reg = registry();
        assert_eq!(reg.command_line(&sb, &call("nope", json!({}))), None);
        assert_eq!(
            reg.command_line(&sb, &call("read_file", json!({"wrong": 1}))),
            None
        );
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let dir = TempDir::new("unknown");
        let sb = sandbox(&dir);
        let outcome = registry().execute(&sb, &call("nope", json!({}))).await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[tokio::test]
    async fn bad_arguments_return_error() {
        let dir = TempDir::new("bad-args");
        let sb = sandbox(&dir);
        let outcome = registry()
            .execute(&sb, &call("read_file", json!({"wrong": 1})))
            .await;
        let ToolOutcome::Error(message) = outcome else {
            panic!("expected an error outcome");
        };
        assert!(
            message.contains("invalid arguments"),
            "expected the centralized parse error, got: {message}"
        );
    }

    #[tokio::test]
    async fn read_write_edit_delete_roundtrip() {
        let dir = TempDir::new("roundtrip");
        let sb = sandbox(&dir);
        let reg = registry();

        assert_eq!(
            reg.execute(
                &sb,
                &call(
                    "write_file",
                    json!({"path": "a.txt", "content": "hello world"})
                )
            )
            .await,
            ToolOutcome::Ok("wrote 11 bytes to a.txt".to_string())
        );
        assert_eq!(
            reg.execute(&sb, &call("read_file", json!({"path": "a.txt"})))
                .await,
            ToolOutcome::Ok("hello world".to_string())
        );
        assert_eq!(
            reg.execute(
                &sb,
                &call(
                    "edit_file",
                    json!({"path": "a.txt", "old_string": "world", "new_string": "rust"})
                )
            )
            .await,
            ToolOutcome::Ok("edited a.txt".to_string())
        );
        assert_eq!(
            reg.execute(&sb, &call("read_file", json!({"path": "a.txt"})))
                .await,
            ToolOutcome::Ok("hello rust".to_string())
        );
        assert_eq!(
            reg.execute(&sb, &call("delete_file", json!({"path": "a.txt"})))
                .await,
            ToolOutcome::Ok("deleted a.txt".to_string())
        );
        assert!(!sb.root().join("a.txt").exists());
    }

    #[tokio::test]
    async fn write_file_round_trips_content_with_shell_metacharacters() {
        // The content reaches `tee` through the child's stdin, never the command line — so newlines,
        // quotes, `$`, and backticks must survive verbatim with no interpolation.
        let dir = TempDir::new("write-special");
        let sb = sandbox(&dir);
        let reg = registry();
        let content = "line1\n\"quoted\" $HOME `whoami`\n$(echo nope)\tend";

        let written = reg
            .execute(
                &sb,
                &call("write_file", json!({"path": "x.sh", "content": content})),
            )
            .await;
        assert!(matches!(written, ToolOutcome::Ok(_)));
        assert_eq!(
            reg.execute(&sb, &call("read_file", json!({"path": "x.sh"})))
                .await,
            ToolOutcome::Ok(content.to_string())
        );
    }

    #[tokio::test]
    async fn read_file_truncates_files_larger_than_the_cap() {
        let dir = TempDir::new("read-cap");
        let sb = sandbox(&dir);
        let big = "a".repeat(READ_FILE_MAX_BYTES + 500);
        fs::write(sb.root().join("big.txt"), big.as_bytes()).unwrap();
        match registry()
            .execute(&sb, &call("read_file", json!({"path": "big.txt"})))
            .await
        {
            ToolOutcome::Ok(text) => assert!(text.contains("truncated at")),
            other => panic!("expected truncated content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_creates_missing_parent_directories() {
        let dir = TempDir::new("write-nested");
        let sb = sandbox(&dir);
        let outcome = registry()
            .execute(
                &sb,
                &call("write_file", json!({"path": "a/b/c.txt", "content": "x"})),
            )
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").join("b").join("c.txt").exists());
    }

    #[tokio::test]
    async fn edit_missing_old_string_returns_error() {
        let dir = TempDir::new("edit-miss");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"hello").unwrap();
        let outcome = registry()
            .execute(
                &sb,
                &call(
                    "edit_file",
                    json!({"path": "a.txt", "old_string": "zzz", "new_string": "y"}),
                ),
            )
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[tokio::test]
    async fn delete_refuses_directories() {
        let dir = TempDir::new("delete-dir");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("sub")).unwrap();
        let outcome = registry()
            .execute(&sb, &call("delete_file", json!({"path": "sub"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().join("sub").is_dir());
    }

    #[tokio::test]
    async fn list_dir_sorts_and_marks_directories() {
        let dir = TempDir::new("list");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("b.txt"), b"x").unwrap();
        fs::create_dir(sb.root().join("a_dir")).unwrap();
        let outcome = registry().execute(&sb, &call("list_dir", json!({}))).await;
        assert_eq!(outcome, ToolOutcome::Ok("a_dir/\nb.txt".to_string()));
    }

    #[tokio::test]
    async fn search_finds_substring_recursively() {
        let dir = TempDir::new("search");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("sub")).unwrap();
        fs::write(
            sb.root().join("sub").join("f.txt"),
            b"alpha\nNEEDLE here\nbeta",
        )
        .unwrap();
        let outcome = registry()
            .execute(&sb, &call("search", json!({"query": "NEEDLE"})))
            .await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(text.contains("sub/f.txt:2:"));
                assert!(text.contains("NEEDLE here"));
            }
            other => panic!("expected matches, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_skips_binary_files() {
        let dir = TempDir::new("search-binary");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("bin"), [b'N', b'E', b'E', 0, b'D']).unwrap();
        let outcome = registry()
            .execute(&sb, &call("search", json!({"query": "NEE"})))
            .await;
        assert_eq!(outcome, ToolOutcome::Ok("no matches".to_string()));
    }

    #[tokio::test]
    async fn move_path_relocates_a_file() {
        let dir = TempDir::new("move-file");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"data").unwrap();
        assert_eq!(
            registry()
                .execute(
                    &sb,
                    &call(
                        "move_path",
                        json!({"source": "a.txt", "destination": "b.txt"})
                    )
                )
                .await,
            ToolOutcome::Ok("moved a.txt to b.txt".to_string())
        );
        assert!(!sb.root().join("a.txt").exists());
        assert_eq!(fs::read_to_string(sb.root().join("b.txt")).unwrap(), "data");
    }

    #[tokio::test]
    async fn move_path_relocates_a_directory_with_contents() {
        let dir = TempDir::new("move-dir");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("src")).unwrap();
        fs::write(sb.root().join("src").join("f.txt"), b"x").unwrap();
        let outcome = registry()
            .execute(
                &sb,
                &call("move_path", json!({"source": "src", "destination": "dst"})),
            )
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(!sb.root().join("src").exists());
        assert!(sb.root().join("dst").join("f.txt").exists());
    }

    #[tokio::test]
    async fn move_path_creates_missing_destination_dirs() {
        let dir = TempDir::new("move-mkdir");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"x").unwrap();
        let outcome = registry()
            .execute(
                &sb,
                &call(
                    "move_path",
                    json!({"source": "a.txt", "destination": "nested/deep/b.txt"}),
                ),
            )
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("nested").join("deep").join("b.txt").exists());
    }

    #[tokio::test]
    async fn move_path_missing_source_returns_error() {
        let dir = TempDir::new("move-missing");
        let sb = sandbox(&dir);
        let outcome = registry()
            .execute(
                &sb,
                &call(
                    "move_path",
                    json!({"source": "nope.txt", "destination": "b.txt"}),
                ),
            )
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[tokio::test]
    async fn move_path_refuses_to_move_the_root() {
        let dir = TempDir::new("move-root");
        let sb = sandbox(&dir);
        let outcome = registry()
            .execute(
                &sb,
                &call("move_path", json!({"source": ".", "destination": "x"})),
            )
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[tokio::test]
    async fn create_dir_creates_nested_directories() {
        let dir = TempDir::new("create-dir");
        let sb = sandbox(&dir);
        let outcome = registry()
            .execute(&sb, &call("create_dir", json!({"path": "a/b/c"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").join("b").join("c").is_dir());
    }

    #[tokio::test]
    async fn create_dir_is_idempotent_for_existing_directory() {
        let dir = TempDir::new("create-dir-exists");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("a")).unwrap();
        let outcome = registry()
            .execute(&sb, &call("create_dir", json!({"path": "a"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").is_dir());
    }

    #[tokio::test]
    async fn delete_dir_removes_a_directory_recursively() {
        let dir = TempDir::new("delete-dir");
        let sb = sandbox(&dir);
        fs::create_dir_all(sb.root().join("a").join("b")).unwrap();
        fs::write(sb.root().join("a").join("b").join("f.txt"), b"x").unwrap();
        let outcome = registry()
            .execute(&sb, &call("delete_dir", json!({"path": "a"})))
            .await;
        assert_eq!(outcome, ToolOutcome::Ok("deleted directory a".to_string()));
        assert!(!sb.root().join("a").exists());
    }

    #[tokio::test]
    async fn delete_dir_refuses_a_file() {
        let dir = TempDir::new("delete-dir-file");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"x").unwrap();
        let outcome = registry()
            .execute(&sb, &call("delete_dir", json!({"path": "a.txt"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().join("a.txt").exists());
    }

    #[tokio::test]
    async fn delete_dir_refuses_the_root() {
        let dir = TempDir::new("delete-dir-root");
        let sb = sandbox(&dir);
        let outcome = registry()
            .execute(&sb, &call("delete_dir", json!({"path": "."})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().is_dir());
    }

    #[tokio::test]
    async fn execute_edits_file_outside_workspace() {
        let outside = TempDir::new("edit-abs");
        let file = outside.path.join("f.txt");
        fs::write(&file, b"hello world").unwrap();
        let dir = TempDir::new("edit-abs-inside");
        let sb = sandbox(&dir);
        let outcome = registry().execute(
            &sb,
            &call(
                "edit_file",
                json!({ "path": file.to_str().unwrap(), "old_string": "world", "new_string": "rust" }),
            ),
        ).await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello rust");
    }
}
