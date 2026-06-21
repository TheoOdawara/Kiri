use std::fs;
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::models::tools::{FunctionDef, Tool, ToolKind};
use crate::modules::tools::infrastructure::sandbox::{
    CreateResolution, Sandbox, is_absolute_target,
};
use crate::shared::kernel::tool_call::ToolCall;

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
            "Read a UTF-8 text file and return its contents. Paths are relative to the active workspace \
             root; an absolute path or '~/…' may reach outside it (the user confirms each call). '..' \
             in a relative path is rejected.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Path relative to the active workspace root, or an absolute / ~ path to reach outside it." } }
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
                    "path": { "type": "string", "description": "Path relative to the active workspace root, or an absolute / ~ path to reach outside it." },
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
                    "path": { "type": "string", "description": "Path relative to the active workspace root, or an absolute / ~ path to reach outside it." },
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
                "properties": { "path": { "type": "string", "description": "Path relative to the active workspace root, or an absolute / ~ path to reach outside it." } }
            }),
        ),
        function(
            "move_path",
            "Move or rename a file or directory. Creates missing parent directories of the destination \
             (with user confirmation) and overwrites an existing destination (with confirmation). Both \
             paths are relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["source", "destination"],
                "properties": {
                    "source": { "type": "string", "description": "Existing file or directory to move, relative to the workspace root." },
                    "destination": { "type": "string", "description": "New path, relative to the workspace root." }
                }
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
            "create_dir",
            "Create a directory, including any missing parent directories. The path is relative to the \
             workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Directory path to create, relative to the workspace root." } }
            }),
        ),
        function(
            "delete_dir",
            "Delete a directory and all of its contents, recursively. Requires confirmation. The path \
             is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Directory path to delete, relative to the workspace root." } }
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
        "move_path" => run_move(sandbox, args),
        "list_dir" => run_list(sandbox, args),
        "create_dir" => run_create_dir(sandbox, args),
        "delete_dir" => run_delete_dir(sandbox, args),
        "search" => run_search(sandbox, args),
        other => ToolOutcome::Error(format!("unknown tool '{other}'")),
    }
}

/// A confirmation request: the line to show and whether Enter approves it. Operations inside the active
/// workspace default to accept (`[S/n]`); operations on an explicit absolute/`~` path (potentially
/// outside the workspace) default to decline (`[s/N]`), requiring a deliberate "yes".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub prompt: String,
    pub default_accept: bool,
}

/// The prompt shown before a tool call runs. The CLI confirms *every* call — reads included — so this
/// returns `Some` for any tool whose arguments parse; only unparseable arguments yield `None`, letting
/// `execute` report the error. The only I/O is the sandbox path resolution used to phrase write/move
/// precisely (mkdir vs overwrite vs new); the caller performs the actual prompt.
pub fn confirmation_prompt(sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
    let args = call.function.arguments.as_str();
    let (action, primary): (String, String) = match call.function.name.as_str() {
        "read_file" => {
            let a: PathArgs = parse(args).ok()?;
            (format!("Ler '{}'?", a.path), a.path)
        }
        "list_dir" => {
            let a: ListArgs = parse(args).ok()?;
            (format!("Listar '{}'?", a.path), a.path)
        }
        "search" => {
            let a: SearchArgs = parse(args).ok()?;
            (format!("Buscar '{}' em '{}'?", a.query, a.path), a.path)
        }
        "write_file" => {
            let a: PathArgs = parse(args).ok()?;
            let action = match sandbox.resolve_create(&a.path) {
                Ok(r) if !r.missing_dirs.is_empty() => format!(
                    "Criar diretório(s) '{}' e gravar '{}'?",
                    missing_dirs_label(&r, sandbox),
                    a.path
                ),
                Ok(r) if r.target.exists() => format!("Sobrescrever '{}'?", a.path),
                _ => format!("Criar e gravar '{}'?", a.path),
            };
            (action, a.path)
        }
        "edit_file" => {
            let a: PathArgs = parse(args).ok()?;
            (format!("Editar '{}'?", a.path), a.path)
        }
        "create_dir" => {
            let a: PathArgs = parse(args).ok()?;
            (format!("Criar diretório '{}'?", a.path), a.path)
        }
        "move_path" => {
            let a: MoveArgs = parse(args).ok()?;
            let action = match sandbox.resolve_create(&a.destination) {
                Ok(r) if !r.missing_dirs.is_empty() => format!(
                    "Criar diretório(s) '{}' e mover '{}' → '{}'?",
                    missing_dirs_label(&r, sandbox),
                    a.source,
                    a.destination
                ),
                Ok(r) if r.target.exists() => {
                    format!(
                        "Sobrescrever '{}' movendo de '{}'?",
                        a.destination, a.source
                    )
                }
                _ => format!("Mover '{}' → '{}'?", a.source, a.destination),
            };
            (action, a.destination)
        }
        "delete_file" => {
            let a: PathArgs = parse(args).ok()?;
            (format!("Excluir '{}'?", a.path), a.path)
        }
        "delete_dir" => {
            let a: PathArgs = parse(args).ok()?;
            (
                format!(
                    "Excluir RECURSIVAMENTE o diretório '{}' e TODO o seu conteúdo?",
                    a.path
                ),
                a.path,
            )
        }
        _ => return None,
    };
    let default_accept = !is_absolute_target(&primary);
    let suffix = if default_accept { "[S/n]" } else { "[s/N]" };
    Some(Confirmation {
        prompt: format!("{action} {suffix} "),
        default_accept,
    })
}

/// Workspace-relative, comma-joined list of the directories a create/move would have to make.
fn missing_dirs_label(resolution: &CreateResolution, sandbox: &Sandbox) -> String {
    resolution
        .missing_dirs
        .iter()
        .map(|dir| {
            dir.strip_prefix(sandbox.root())
                .unwrap_or(dir)
                .display()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(", ")
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
struct MoveArgs {
    source: String,
    destination: String,
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

fn run_move(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: MoveArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    let source = match sandbox.resolve_existing(&args.source) {
        Ok(source) => source,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    if source == sandbox.root() {
        return ToolOutcome::Error("refusing to move the workspace root".to_string());
    }
    let resolution = match sandbox.resolve_create(&args.destination) {
        Ok(resolution) => resolution,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    if !resolution.missing_dirs.is_empty()
        && let Some(parent) = resolution.target.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return ToolOutcome::Error(format!(
            "cannot create directories for {}: {error}",
            args.destination
        ));
    }
    match fs::rename(&source, &resolution.target) {
        Ok(()) => ToolOutcome::Ok(format!("moved {} to {}", args.source, args.destination)),
        Err(error) => ToolOutcome::Error(format!(
            "cannot move {} to {}: {error}",
            args.source, args.destination
        )),
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

fn run_create_dir(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: PathArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    let resolution = match sandbox.resolve_create(&args.path) {
        Ok(resolution) => resolution,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    if resolution.target.is_dir() {
        return ToolOutcome::Ok(format!("directory already exists: {}", args.path));
    }
    match fs::create_dir_all(&resolution.target) {
        Ok(()) => ToolOutcome::Ok(format!("created directory {}", args.path)),
        Err(error) => ToolOutcome::Error(format!("cannot create {}: {error}", args.path)),
    }
}

fn run_delete_dir(sandbox: &Sandbox, args: &str) -> ToolOutcome {
    let args: PathArgs = match parse(args) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
    };
    let path = match sandbox.resolve_existing(&args.path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::Error(error.to_string()),
    };
    if path == sandbox.root() {
        return ToolOutcome::Error("refusing to delete the workspace root".to_string());
    }
    match fs::metadata(&path) {
        Ok(metadata) if !metadata.is_dir() => {
            return ToolOutcome::Error(format!(
                "{} is not a directory; use delete_file",
                args.path
            ));
        }
        Ok(_) => {}
        Err(error) => return ToolOutcome::Error(format!("cannot stat {}: {error}", args.path)),
    }
    match fs::remove_dir_all(&path) {
        Ok(()) => ToolOutcome::Ok(format!("deleted directory {}", args.path)),
        Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
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
    // Bound the recursion (and the displayed paths) to the resolved start directory, so a search begun
    // outside the active workspace stays within its own subtree.
    let base = start.clone();
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
            if !path.starts_with(&base) {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                search_file(&path, &args.query, &base, &mut matches);
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
    use crate::shared::kernel::tool_call::FunctionCall;
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
    fn tool_definitions_expose_all_tools() {
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
    fn move_path_relocates_a_file() {
        let dir = TempDir::new("move-file");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"data").unwrap();
        assert_eq!(
            execute(
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
        let outcome = execute(
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
        let outcome = execute(
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
        let outcome = execute(
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
        let outcome = execute(
            &sb,
            &call("move_path", json!({"source": ".", "destination": "x"})),
        );
        assert!(matches!(outcome, ToolOutcome::Error(_)));
    }

    #[test]
    fn create_dir_creates_nested_directories() {
        let dir = TempDir::new("create-dir");
        let sb = sandbox(&dir);
        let outcome = execute(&sb, &call("create_dir", json!({"path": "a/b/c"})));
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").join("b").join("c").is_dir());
    }

    #[test]
    fn create_dir_is_idempotent_for_existing_directory() {
        let dir = TempDir::new("create-dir-exists");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("a")).unwrap();
        let outcome = execute(&sb, &call("create_dir", json!({"path": "a"})));
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(sb.root().join("a").is_dir());
    }

    #[test]
    fn delete_dir_removes_a_directory_recursively() {
        let dir = TempDir::new("delete-dir");
        let sb = sandbox(&dir);
        fs::create_dir_all(sb.root().join("a").join("b")).unwrap();
        fs::write(sb.root().join("a").join("b").join("f.txt"), b"x").unwrap();
        let outcome = execute(&sb, &call("delete_dir", json!({"path": "a"})));
        assert_eq!(outcome, ToolOutcome::Ok("deleted directory a".to_string()));
        assert!(!sb.root().join("a").exists());
    }

    #[test]
    fn delete_dir_refuses_a_file() {
        let dir = TempDir::new("delete-dir-file");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"x").unwrap();
        let outcome = execute(&sb, &call("delete_dir", json!({"path": "a.txt"})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().join("a.txt").exists());
    }

    #[test]
    fn delete_dir_refuses_the_root() {
        let dir = TempDir::new("delete-dir-root");
        let sb = sandbox(&dir);
        let outcome = execute(&sb, &call("delete_dir", json!({"path": "."})));
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(sb.root().is_dir());
    }

    #[test]
    fn confirmation_prompts_for_read_list_search() {
        let dir = TempDir::new("confirm-reads");
        let sb = sandbox(&dir);
        let read = confirmation_prompt(&sb, &call("read_file", json!({"path": "a.txt"}))).unwrap();
        assert!(read.prompt.contains("Ler"));
        assert!(read.prompt.ends_with("[S/n] "));
        assert!(read.default_accept);
        assert!(
            confirmation_prompt(&sb, &call("list_dir", json!({})))
                .unwrap()
                .prompt
                .contains("Listar")
        );
        assert!(
            confirmation_prompt(&sb, &call("search", json!({"query": "x"})))
                .unwrap()
                .prompt
                .contains("Buscar")
        );
    }

    #[test]
    fn confirmation_prompts_for_new_file() {
        let dir = TempDir::new("confirm-create");
        let sb = sandbox(&dir);
        assert!(
            confirmation_prompt(
                &sb,
                &call("write_file", json!({"path": "new.txt", "content": "x"}))
            )
            .unwrap()
            .prompt
            .contains("Criar e gravar")
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
        assert!(prompt.unwrap().prompt.contains("Sobrescrever"));
    }

    #[test]
    fn confirmation_some_for_missing_dirs() {
        let dir = TempDir::new("confirm-mkdir");
        let sb = sandbox(&dir);
        let prompt = confirmation_prompt(
            &sb,
            &call("write_file", json!({"path": "a/b/c.txt", "content": "x"})),
        )
        .unwrap();
        assert!(prompt.prompt.contains("Criar diretório"));
        assert!(prompt.prompt.contains("a/b"));
    }

    #[test]
    fn confirmation_some_for_delete_and_edit_when_file_exists() {
        let dir = TempDir::new("confirm-destructive");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"data").unwrap();
        assert!(
            confirmation_prompt(&sb, &call("delete_file", json!({"path": "a.txt"})))
                .unwrap()
                .prompt
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
            .prompt
            .contains("Editar")
        );
    }

    #[test]
    fn confirmation_prompts_for_clean_move_and_create_dir() {
        let dir = TempDir::new("confirm-move-clean");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"x").unwrap();
        assert!(
            confirmation_prompt(
                &sb,
                &call(
                    "move_path",
                    json!({"source": "a.txt", "destination": "b.txt"})
                )
            )
            .unwrap()
            .prompt
            .contains("Mover")
        );
        assert!(
            confirmation_prompt(&sb, &call("create_dir", json!({"path": "newdir"})))
                .unwrap()
                .prompt
                .contains("Criar diretório")
        );
    }

    #[test]
    fn confirmation_some_for_move_overwrite_and_mkdir() {
        let dir = TempDir::new("confirm-move");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("a.txt"), b"x").unwrap();
        fs::write(sb.root().join("b.txt"), b"old").unwrap();
        assert!(
            confirmation_prompt(
                &sb,
                &call(
                    "move_path",
                    json!({"source": "a.txt", "destination": "b.txt"})
                )
            )
            .unwrap()
            .prompt
            .contains("Sobrescrever")
        );
        let prompt = confirmation_prompt(
            &sb,
            &call(
                "move_path",
                json!({"source": "a.txt", "destination": "x/y/c.txt"}),
            ),
        )
        .unwrap();
        assert!(prompt.prompt.contains("Criar diretório"));
    }

    #[test]
    fn confirmation_some_for_delete_dir_recursive() {
        let dir = TempDir::new("confirm-delete-dir");
        let sb = sandbox(&dir);
        fs::create_dir(sb.root().join("d")).unwrap();
        assert!(
            confirmation_prompt(&sb, &call("delete_dir", json!({"path": "d"})))
                .unwrap()
                .prompt
                .contains("RECURSIVAMENTE")
        );
    }

    #[test]
    fn confirmation_defaults_decline_for_absolute_path() {
        let outside = TempDir::new("confirm-abs");
        let file = outside.path.join("f.txt");
        fs::write(&file, b"x").unwrap();
        let dir = TempDir::new("confirm-abs-inside");
        let sb = sandbox(&dir);
        let c = confirmation_prompt(
            &sb,
            &call("read_file", json!({ "path": file.to_str().unwrap() })),
        )
        .unwrap();
        assert!(!c.default_accept);
        assert!(c.prompt.ends_with("[s/N] "));
        assert!(c.prompt.contains(file.to_str().unwrap()));
    }

    #[test]
    fn execute_edits_file_outside_workspace() {
        let outside = TempDir::new("edit-abs");
        let file = outside.path.join("f.txt");
        fs::write(&file, b"hello world").unwrap();
        let dir = TempDir::new("edit-abs-inside");
        let sb = sandbox(&dir);
        let outcome = execute(
            &sb,
            &call(
                "edit_file",
                json!({ "path": file.to_str().unwrap(), "old_string": "world", "new_string": "rust" }),
            ),
        );
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello rust");
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
