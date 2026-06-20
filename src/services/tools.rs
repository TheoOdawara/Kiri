use std::fs;
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::models::tools::{FunctionDef, Tool, ToolCall, ToolKind};
use crate::services::sandbox::Sandbox;

const READ_FILE_MAX_BYTES: usize = 64 * 1024;
const EDIT_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const SEARCH_FILE_MAX_BYTES: u64 = 1024 * 1024;
const SEARCH_MAX_MATCHES: usize = 100;
const SEARCH_MAX_LINE_CHARS: usize = 200;
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// The result of executing a tool. Failures are data the model reads and recovers from — never panics
/// nor `Err` that would abort the agentic turn.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Ok(String),
    Error(String),
    Declined,
}

impl ToolOutcome {
    /// The content placed in the `role: tool` message returned to the model.
    pub fn into_message_content(self) -> String {
        match self {
            ToolOutcome::Ok(text) => text,
            ToolOutcome::Error(error) => format!("error: {error}"),
            ToolOutcome::Declined => "declined by user".to_string(),
        }
    }
}

/// The file tools advertised to the model. Paths are always relative to the sandbox root.
pub fn tool_definitions() -> Vec<Tool> {
    vec![
        function(
            "read_file",
            "Read a UTF-8 text file and return its contents. The path is relative to the workspace \
             root; '..' and absolute paths are rejected.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Path relative to the workspace root." } }
            }),
        ),
        function(
            "write_file",
            "Create a file or overwrite it with the full given content. Creates missing parent \
             directories (with user confirmation). The path is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the workspace root." },
                    "content": { "type": "string", "description": "Full file content to write." }
                }
            }),
        ),
        function(
            "edit_file",
            "Replace the first exact occurrence of old_string with new_string in an existing file.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "old_string", "new_string"],
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the workspace root." },
                    "old_string": { "type": "string", "description": "Exact text to find (must be unique enough)." },
                    "new_string": { "type": "string", "description": "Replacement text." }
                }
            }),
        ),
        function(
            "delete_file",
            "Delete a file (not a directory). The path is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Path relative to the workspace root." } }
            }),
        ),
        function(
            "list_dir",
            "List the entries of a directory (one level). Defaults to the workspace root. Directories \
             are suffixed with '/'.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "path": { "type": "string", "description": "Directory path relative to the workspace root. Defaults to '.'." } }
            }),
        ),
        function(
            "search",
            "Recursively search file contents under a directory for a plain substring (case-sensitive). \
             Binary files are skipped. Defaults to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "description": "Substring to search for." },
                    "path": { "type": "string", "description": "Directory to search under, relative to the workspace root. Defaults to '.'." }
                }
            }),
        ),
    ]
}

fn function(name: &str, description: &str, parameters: serde_json::Value) -> Tool {
    Tool {
        kind: ToolKind::Function,
        function: FunctionDef {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

/// Run a tool call against the sandbox. Every path goes through the sandbox chokepoint.
pub fn execute(sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
    let args = call.function.arguments.as_str();
    match call.function.name.as_str() {
        "read_file" => run_read(sandbox, args),
        "write_file" => run_write(sandbox, args),
        "edit_file" => run_edit(sandbox, args),
        "delete_file" => run_delete(sandbox, args),
        "list_dir" => run_list(sandbox, args),
        "search" => run_search(sandbox, args),
        other => ToolOutcome::Error(format!("unknown tool '{other}'")),
    }
}

/// The prompt to show before a destructive operation, or `None` when the call needs no confirmation
/// (read/list/search, a plain create in an existing directory, or unparseable arguments — which
/// `execute` will report as an error). No I/O happens here; the caller performs the actual prompt.
pub fn confirmation_prompt(sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
    match call.function.name.as_str() {
        "delete_file" => {
            let args: PathArgs = parse(&call.function.arguments).ok()?;
            sandbox.resolve_existing(&args.path).ok()?;
            Some(format!("Excluir '{}'? [s/N] ", args.path))
        }
        "edit_file" => {
            let args: PathArgs = parse(&call.function.arguments).ok()?;
            sandbox.resolve_existing(&args.path).ok()?;
            Some(format!("Editar '{}'? [s/N] ", args.path))
        }
        "write_file" => {
            let args: PathArgs = parse(&call.function.arguments).ok()?;
            let resolution = sandbox.resolve_create(&args.path).ok()?;
            if !resolution.missing_dirs.is_empty() {
                let dirs = resolution
                    .missing_dirs
                    .iter()
                    .map(|dir| {
                        dir.strip_prefix(sandbox.root())
                            .unwrap_or(dir)
                            .display()
                            .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "Criar diretório(s) '{dirs}' e gravar '{}'? [s/N] ",
                    args.path
                ))
            } else if resolution.target.exists() {
                Some(format!("Sobrescrever '{}'? [s/N] ", args.path))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct PathArgs {
    path: String,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct EditArgs {
    path: String,
    old_string: String,
    new_string: String,
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default = "dot")]
    path: String,
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "dot")]
    path: String,
}

fn dot() -> String {
    ".".to_string()
}

fn parse<T: serde::de::DeserializeOwned>(args: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(args)
}

/// Read at most `cap` bytes from `path`, bounding allocation against very large files.
fn read_capped(path: &Path, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    fs::File::open(path)?
        .take(cap as u64)
        .read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn run_read(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: PathArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
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

fn run_write(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: WriteArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    let resolution = match sandbox.resolve_create(&args.path) {
        Ok(resolution) => resolution,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    if !resolution.missing_dirs.is_empty()
        && let Some(parent) = resolution.target.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return ToolOutcome::Error(format!(
            "cannot create directories for {}: {error}",
            args.path
        ));
    }
    match fs::write(&resolution.target, args.content.as_bytes()) {
        Ok(()) => ToolOutcome::Ok(format!(
            "wrote {} bytes to {}",
            args.content.len(),
            args.path
        )),
        Err(error) => ToolOutcome::Error(format!("cannot write {}: {error}", args.path)),
    }
}

fn run_edit(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: EditArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    if args.old_string.is_empty() {
        return ToolOutcome::Error("old_string must not be empty".to_string());
    }
    let path = match sandbox.resolve_existing(&args.path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    match fs::metadata(&path) {
        Ok(metadata) if metadata.len() > EDIT_FILE_MAX_BYTES => {
            return ToolOutcome::Error(format!(
                "{} is too large to edit (max {EDIT_FILE_MAX_BYTES} bytes)",
                args.path
            ));
        }
        Ok(_) => {}
        Err(error) => return ToolOutcome::Error(format!("cannot stat {}: {error}", args.path)),
    }
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            return ToolOutcome::Error(format!("cannot read {} as text: {error}", args.path));
        }
    };
    let Some(position) = content.find(&args.old_string) else {
        return ToolOutcome::Error(format!("old_string not found in {}", args.path));
    };
    let mut updated = String::with_capacity(content.len() + args.new_string.len());
    updated.push_str(&content[..position]);
    updated.push_str(&args.new_string);
    updated.push_str(&content[position + args.old_string.len()..]);
    match fs::write(&path, updated.as_bytes()) {
        Ok(()) => ToolOutcome::Ok(format!("edited {}", args.path)),
        Err(error) => ToolOutcome::Error(format!("cannot write {}: {error}", args.path)),
    }
}

fn run_delete(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: PathArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    let path = match sandbox.resolve_existing(&args.path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            return ToolOutcome::Error(format!("{} is a directory; not deleted", args.path));
        }
        Ok(_) => {}
        Err(error) => return ToolOutcome::Error(format!("cannot stat {}: {error}", args.path)),
    }
    match fs::remove_file(&path) {
        Ok(()) => ToolOutcome::Ok(format!("deleted {}", args.path)),
        Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
    }
}

fn run_list(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: ListArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    let dir = match sandbox.resolve_existing(&args.path) {
        Ok(dir) => dir,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => return ToolOutcome::Error(format!("cannot list {}: {error}", args.path)),
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

fn run_search(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: SearchArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    if args.query.is_empty() {
        return ToolOutcome::Error("query must not be empty".to_string());
    }
    let start = match sandbox.resolve_existing(&args.path) {
        Ok(start) => start,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };

    let mut matches: Vec<String> = Vec::new();
    let mut stack = vec![start];
    let mut truncated = false;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue; // never follow symlinks: avoids escape and traversal loops
            }
            let path = entry.path();
            if !path.starts_with(sandbox.root()) {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                search_file(&path, &args.query, sandbox.root(), &mut matches);
                if matches.len() >= SEARCH_MAX_MATCHES {
                    truncated = true;
                    break;
                }
            }
        }
        if truncated {
            break;
        }
    }

    if matches.is_empty() {
        return ToolOutcome::Ok("no matches".to_string());
    }
    let mut output = matches.join("\n");
    if truncated {
        output.push_str(&format!("\n… (truncated at {SEARCH_MAX_MATCHES} matches)"));
    }
    ToolOutcome::Ok(output)
}

fn search_file(path: &Path, query: &str, root: &Path, matches: &mut Vec<String>) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > SEARCH_FILE_MAX_BYTES => return, // skip large files
        Ok(_) => {}
        Err(_) => return,
    }
    let Ok(bytes) = read_capped(path, SEARCH_FILE_MAX_BYTES as usize) else {
        return;
    };
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return; // treat NUL-containing files as binary and skip
    }
    let text = String::from_utf8_lossy(&bytes);
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    for (number, line) in text.lines().enumerate() {
        if matches.len() >= SEARCH_MAX_MATCHES {
            return;
        }
        if line.contains(query) {
            let shown: String = line.chars().take(SEARCH_MAX_LINE_CHARS).collect();
            matches.push(format!("{relative}:{}: {shown}", number + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::tools::FunctionCall;
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
    fn tool_definitions_expose_the_six_tools() {
        let names: Vec<String> = tool_definitions()
            .into_iter()
            .map(|tool| tool.function.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "list_dir",
                "search"
            ]
        );
    }

    #[test]
    fn unknown_tool_returns_error() {
        let dir = TempDir::new("unknown");
        let sb = sandbox(&dir);
        let outcome = execute(&sb, &call("nope", json!({})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[test]
    fn bad_arguments_return_error() {
        let dir = TempDir::new("bad-args");
        let sb = sandbox(&dir);
        let outcome = execute(&sb, &call("read_file", json!({"wrong": 1})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[test]
    fn read_write_edit_delete_roundtrip() {
        let dir = TempDir::new("roundtrip");
        let sb = sandbox(&dir);

        assert_eq!(
            execute(
                &sb,
                &call(
                    "write_file",
                    json!({"path": "a.txt", "content": "hello world"})
                )
            ),
            ToolOutcome::Ok("wrote 11 bytes to a.txt".to_string())
        );
        assert_eq!(
            execute(&sb, &call("read_file", json!({"path": "a.txt"}))),
            ToolOutcome::Ok("hello world".to_string())
        );
        assert_eq!(
            execute(
                &sb,
                &call(
                    "edit_file",
                    json!({"path": "a.txt", "old_string": "world", "new_string": "rust"})
                )
            ),
            ToolOutcome::Ok("edited a.txt".to_string())
        );
        assert_eq!(
            execute(&sb, &call("read_file", json!({"path": "a.txt"}))),
            ToolOutcome::Ok("hello rust".to_string())
        );
        assert_eq!(
            execute(&sb, &call("delete_file", json!({"path": "a.txt"}))),
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
        match execute(&sb, &call("read_file", json!({"path": "big.txt"}))) {
            ToolOutcome::Ok(text) => assert!(text.contains("truncated at")),
            other => panic!("expected truncated content, got {other:?}"),
        }
    }

    #[test]
    fn write_creates_missing_parent_directories() {
        let dir = TempDir::new("write-nested");
        let sb = sandbox(&dir);
        let outcome = execute(
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
        let outcome = execute(
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
        let outcome = execute(&sb, &call("delete_file", json!({"path": "sub"})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().join("sub").is_dir());
    }

    #[test]
    fn list_dir_sorts_and_marks_directories() {
        let dir = TempDir::new("list");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("b.txt"), b"x").unwrap();
        fs::create_dir(sb.root().join("a_dir")).unwrap();
        let outcome = execute(&sb, &call("list_dir", json!({})));
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
        let outcome = execute(&sb, &call("search", json!({"query": "NEEDLE"})));
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
        let outcome = execute(&sb, &call("search", json!({"query": "NEE"})));
        assert_eq!(outcome, ToolOutcome::Ok("no matches".to_string()));
    }

    #[test]
    fn confirmation_none_for_read_list_search() {
        let dir = TempDir::new("confirm-none");
        let sb = sandbox(&dir);
        assert!(confirmation_prompt(&sb, &call("read_file", json!({"path": "a.txt"}))).is_none());
        assert!(confirmation_prompt(&sb, &call("list_dir", json!({}))).is_none());
        assert!(confirmation_prompt(&sb, &call("search", json!({"query": "x"}))).is_none());
    }

    #[test]
    fn confirmation_none_for_create_in_existing_dir() {
        let dir = TempDir::new("confirm-create");
        let sb = sandbox(&dir);
        assert!(
            confirmation_prompt(
                &sb,
                &call("write_file", json!({"path": "new.txt", "content": "x"}))
            )
            .is_none()
        );
    }

    #[test]
    fn confirmation_some_for_overwrite() {
        let dir = TempDir::new("confirm-overwrite");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"old").unwrap();
        let prompt = confirmation_prompt(
            &sb,
            &call("write_file", json!({"path": "a.txt", "content": "new"})),
        );
        assert!(prompt.unwrap().contains("Sobrescrever"));
    }

    #[test]
    fn confirmation_some_for_missing_dirs() {
        let dir = TempDir::new("confirm-mkdir");
        let sb = sandbox(&dir);
        let prompt = confirmation_prompt(
            &sb,
            &call("write_file", json!({"path": "a/b/c.txt", "content": "x"})),
        );
        let prompt = prompt.unwrap();
        assert!(prompt.contains("Criar diretório"));
        assert!(prompt.contains("a/b"));
    }

    #[test]
    fn confirmation_some_for_delete_and_edit_when_file_exists() {
        let dir = TempDir::new("confirm-destructive");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"data").unwrap();
        assert!(
            confirmation_prompt(&sb, &call("delete_file", json!({"path": "a.txt"})))
                .unwrap()
                .contains("Excluir")
        );
        assert!(
            confirmation_prompt(
                &sb,
                &call(
                    "edit_file",
                    json!({"path": "a.txt", "old_string": "d", "new_string": "e"})
                )
            )
            .unwrap()
            .contains("Editar")
        );
    }

    #[test]
    fn outcome_into_message_content_maps_variants() {
        assert_eq!(
            ToolOutcome::Ok("ok".to_string()).into_message_content(),
            "ok"
        );
        assert_eq!(
            ToolOutcome::Error("boom".to_string()).into_message_content(),
            "error: boom"
        );
        assert_eq!(
            ToolOutcome::Declined.into_message_content(),
            "declined by user"
        );
    }
}
