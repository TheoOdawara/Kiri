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

    fn find(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .map(|tool| tool.as_ref())
            .find(|tool| tool.name() == name)
    }

    pub fn confirm(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        self.find(&call.function.name)?.confirmation(sandbox, call)
    }

    pub fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        match self.find(&call.function.name) {
            Some(tool) => tool.execute(sandbox, call),
            None => ToolOutcome::Error(format!("unknown tool '{}'", call.function.name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::fs::default_fs_tools;
    use crate::modules::tools::infrastructure::support::READ_FILE_MAX_BYTES;
    use crate::shared::kernel::tool_call::FunctionCall;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
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
        ToolRegistry::new(default_fs_tools())
    }

    fn sandbox(dir: &TempDir) -> Sandbox {
        Sandbox::new(&dir.path).unwrap()
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

    #[test]
    fn schemas_expose_all_tools_in_order() {
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
                "search"
            ]
        );
    }

    #[test]
    fn unknown_tool_returns_error() {
        let dir = TempDir::new("unknown");
        let sb = sandbox(&dir);
        let outcome = registry().execute(&sb, &call("nope", json!({})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[test]
    fn bad_arguments_return_error() {
        let dir = TempDir::new("bad-args");
        let sb = sandbox(&dir);
        let outcome = registry().execute(&sb, &call("read_file", json!({"wrong": 1})));
        let ToolOutcome::Error(message) = outcome else {
            panic!("expected an error outcome");
        };
        assert!(
            message.contains("invalid arguments"),
            "expected the centralized parse error, got: {message}"
        );
    }

    #[test]
    fn read_write_edit_delete_roundtrip() {
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
            ),
            ToolOutcome::Ok("wrote 11 bytes to a.txt".to_string())
        );
        assert_eq!(
            reg.execute(&sb, &call("read_file", json!({"path": "a.txt"}))),
            ToolOutcome::Ok("hello world".to_string())
        );
        assert_eq!(
            reg.execute(
                &sb,
                &call(
                    "edit_file",
                    json!({"path": "a.txt", "old_string": "world", "new_string": "rust"})
                )
            ),
            ToolOutcome::Ok("edited a.txt".to_string())
        );
        assert_eq!(
            reg.execute(&sb, &call("read_file", json!({"path": "a.txt"}))),
            ToolOutcome::Ok("hello rust".to_string())
        );
        assert_eq!(
            reg.execute(&sb, &call("delete_file", json!({"path": "a.txt"}))),
            ToolOutcome::Ok("deleted a.txt".to_string())
        );
        assert!(!sb.root().join("a.txt").exists());
    }

    #[test]
    fn read_file_truncates_files_larger_than_the_cap() {
        let dir = TempDir::new("read-cap");
        let sb = sandbox(&dir);
        let big = "a".repeat(READ_FILE_MAX_BYTES + 500);
        fs::write(sb.root().join("big.txt"), big.as_bytes()).unwrap();
        match registry().execute(&sb, &call("read_file", json!({"path": "big.txt"}))) {
            ToolOutcome::Ok(text) => assert!(text.contains("truncated at")),
            other => panic!("expected truncated content, got {other:?}"),
        }
    }

    #[test]
    fn write_creates_missing_parent_directories() {
        let dir = TempDir::new("write-nested");
        let sb = sandbox(&dir);
        let outcome = registry().execute(
            &sb,
            &call("write_file", json!({"path": "a/b/c.txt", "content": "x"})),
        );
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").join("b").join("c.txt").exists());
    }

    #[test]
    fn edit_missing_old_string_returns_error() {
        let dir = TempDir::new("edit-miss");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"hello").unwrap();
        let outcome = registry().execute(
            &sb,
            &call(
                "edit_file",
                json!({"path": "a.txt", "old_string": "zzz", "new_string": "y"}),
            ),
        );
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[test]
    fn delete_refuses_directories() {
        let dir = TempDir::new("delete-dir");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("sub")).unwrap();
        let outcome = registry().execute(&sb, &call("delete_file", json!({"path": "sub"})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().join("sub").is_dir());
    }

    #[test]
    fn list_dir_sorts_and_marks_directories() {
        let dir = TempDir::new("list");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("b.txt"), b"x").unwrap();
        fs::create_dir(sb.root().join("a_dir")).unwrap();
        let outcome = registry().execute(&sb, &call("list_dir", json!({})));
        assert_eq!(outcome, ToolOutcome::Ok("a_dir/\nb.txt".to_string()));
    }

    #[test]
    fn search_finds_substring_recursively() {
        let dir = TempDir::new("search");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("sub")).unwrap();
        fs::write(
            sb.root().join("sub").join("f.txt"),
            b"alpha\nNEEDLE here\nbeta",
        )
        .unwrap();
        let outcome = registry().execute(&sb, &call("search", json!({"query": "NEEDLE"})));
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(text.contains("sub/f.txt:2:"));
                assert!(text.contains("NEEDLE here"));
            }
            other => panic!("expected matches, got {other:?}"),
        }
    }

    #[test]
    fn search_skips_binary_files() {
        let dir = TempDir::new("search-binary");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("bin"), [b'N', b'E', b'E', 0, b'D']).unwrap();
        let outcome = registry().execute(&sb, &call("search", json!({"query": "NEE"})));
        assert_eq!(outcome, ToolOutcome::Ok("no matches".to_string()));
    }

    #[test]
    fn move_path_relocates_a_file() {
        let dir = TempDir::new("move-file");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"data").unwrap();
        assert_eq!(
            registry().execute(
                &sb,
                &call(
                    "move_path",
                    json!({"source": "a.txt", "destination": "b.txt"})
                )
            ),
            ToolOutcome::Ok("moved a.txt to b.txt".to_string())
        );
        assert!(!sb.root().join("a.txt").exists());
        assert_eq!(fs::read_to_string(sb.root().join("b.txt")).unwrap(), "data");
    }

    #[test]
    fn move_path_relocates_a_directory_with_contents() {
        let dir = TempDir::new("move-dir");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("src")).unwrap();
        fs::write(sb.root().join("src").join("f.txt"), b"x").unwrap();
        let outcome = registry().execute(
            &sb,
            &call("move_path", json!({"source": "src", "destination": "dst"})),
        );
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(!sb.root().join("src").exists());
        assert!(sb.root().join("dst").join("f.txt").exists());
    }

    #[test]
    fn move_path_creates_missing_destination_dirs() {
        let dir = TempDir::new("move-mkdir");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"x").unwrap();
        let outcome = registry().execute(
            &sb,
            &call(
                "move_path",
                json!({"source": "a.txt", "destination": "nested/deep/b.txt"}),
            ),
        );
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("nested").join("deep").join("b.txt").exists());
    }

    #[test]
    fn move_path_missing_source_returns_error() {
        let dir = TempDir::new("move-missing");
        let sb = sandbox(&dir);
        let outcome = registry().execute(
            &sb,
            &call(
                "move_path",
                json!({"source": "nope.txt", "destination": "b.txt"}),
            ),
        );
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[test]
    fn move_path_refuses_to_move_the_root() {
        let dir = TempDir::new("move-root");
        let sb = sandbox(&dir);
        let outcome = registry().execute(
            &sb,
            &call("move_path", json!({"source": ".", "destination": "x"})),
        );
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[test]
    fn create_dir_creates_nested_directories() {
        let dir = TempDir::new("create-dir");
        let sb = sandbox(&dir);
        let outcome = registry().execute(&sb, &call("create_dir", json!({"path": "a/b/c"})));
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").join("b").join("c").is_dir());
    }

    #[test]
    fn create_dir_is_idempotent_for_existing_directory() {
        let dir = TempDir::new("create-dir-exists");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("a")).unwrap();
        let outcome = registry().execute(&sb, &call("create_dir", json!({"path": "a"})));
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").is_dir());
    }

    #[test]
    fn delete_dir_removes_a_directory_recursively() {
        let dir = TempDir::new("delete-dir");
        let sb = sandbox(&dir);
        fs::create_dir_all(sb.root().join("a").join("b")).unwrap();
        fs::write(sb.root().join("a").join("b").join("f.txt"), b"x").unwrap();
        let outcome = registry().execute(&sb, &call("delete_dir", json!({"path": "a"})));
        assert_eq!(outcome, ToolOutcome::Ok("deleted directory a".to_string()));
        assert!(!sb.root().join("a").exists());
    }

    #[test]
    fn delete_dir_refuses_a_file() {
        let dir = TempDir::new("delete-dir-file");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"x").unwrap();
        let outcome = registry().execute(&sb, &call("delete_dir", json!({"path": "a.txt"})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().join("a.txt").exists());
    }

    #[test]
    fn delete_dir_refuses_the_root() {
        let dir = TempDir::new("delete-dir-root");
        let sb = sandbox(&dir);
        let outcome = registry().execute(&sb, &call("delete_dir", json!({"path": "."})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().is_dir());
    }

    #[test]
    fn execute_edits_file_outside_workspace() {
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
        );
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello rust");
    }
}
