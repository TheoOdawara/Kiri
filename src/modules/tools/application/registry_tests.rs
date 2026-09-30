use super::*;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::FsSandbox;
use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
use crate::modules::tools::infrastructure::support::READ_FILE_MAX_BYTES;
use crate::shared::kernel::tool_call::FunctionCall;
use regex::Regex;
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

fn registry() -> ToolRegistry {
    ToolRegistry::new(default_fs_tools(Arc::from(Vec::<Regex>::new()), false))
}

fn sandbox(dir: &TempDir) -> FsSandbox {
    FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap()
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

/// `present_plan` is a plan-only control tool: present only in plan mode, withheld from
/// `schemas()` (default/auto) so the model can never call it to short-circuit a normal turn.
#[tokio::test]
async fn present_plan_is_plan_only() {
    use crate::modules::tools::infrastructure::control::present_plan::PresentPlan;
    let mut tools = default_fs_tools(Arc::from(Vec::<Regex>::new()), false);
    tools.push(Arc::new(PresentPlan));
    let registry = ToolRegistry::new(tools);

    let names = |schemas: Vec<serde_json::Value>| -> Vec<String> {
        schemas
            .iter()
            .map(|schema| schema["function"]["name"].as_str().unwrap().to_string())
            .collect()
    };

    assert!(
        !names(registry.schemas()).contains(&"present_plan".to_string()),
        "present_plan must be withheld outside plan mode"
    );
    assert!(
        !names(registry.schemas_for(ApprovalMode::Default)).contains(&"present_plan".to_string()),
        "present_plan must be absent in default mode"
    );
    assert!(
        names(registry.schemas_for(ApprovalMode::Plan)).contains(&"present_plan".to_string()),
        "present_plan must be advertised in plan mode"
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    let outcome = registry().execute(&sb, &call("nope", json!({}))).await;
    assert!(matches!(outcome, ToolOutcome::Error(_)));
}

#[tokio::test]
async fn bad_arguments_return_error() {
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    fs::write(sb.root().join("b.txt"), b"x").unwrap();
    fs::create_dir(sb.root().join("a_dir")).unwrap();
    let outcome = registry().execute(&sb, &call("list_dir", json!({}))).await;
    assert_eq!(outcome, ToolOutcome::Ok("a_dir/\nb.txt".to_string()));
}

#[tokio::test]
async fn search_finds_substring_recursively() {
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    // A NUL byte amid text marks the file as binary, so search must skip it (even though the
    // surrounding bytes spell the query "NEE").
    fs::write(sb.root().join("bin"), b"NEE\0D").unwrap();
    let outcome = registry()
        .execute(&sb, &call("search", json!({"query": "NEE"})))
        .await;
    assert_eq!(outcome, ToolOutcome::Ok("no matches".to_string()));
}

#[tokio::test]
async fn move_path_relocates_a_file() {
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    let outcome = registry()
        .execute(&sb, &call("create_dir", json!({"path": "a/b/c"})))
        .await;
    assert!(matches!(outcome, ToolOutcome::Ok(_)));
    assert!(sb.root().join("a").join("b").join("c").is_dir());
}

#[tokio::test]
async fn create_dir_is_idempotent_for_existing_directory() {
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
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
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    let outcome = registry()
        .execute(&sb, &call("delete_dir", json!({"path": "."})))
        .await;
    assert!(matches!(outcome, ToolOutcome::Error(_)));
    assert!(sb.root().is_dir());
}

#[tokio::test]
async fn execute_edits_file_outside_workspace() {
    let outside = TempDir::new().unwrap();
    let file = outside.path().join("f.txt");
    fs::write(&file, b"hello world").unwrap();
    let dir = TempDir::new().unwrap();
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
