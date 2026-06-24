use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema, simple_command,
};
use crate::modules::tools::infrastructure::args::{SearchArgs, parse, parse_args};
use crate::modules::tools::infrastructure::sandbox::{Sandbox, default_accept_for};
use crate::modules::tools::infrastructure::support::{SEARCH_MAX_MATCHES, search_file};
use crate::shared::kernel::tool_call::ToolCall;

pub struct Search;

#[async_trait::async_trait(?Send)]
impl Tool for Search {
    fn name(&self) -> &'static str {
        "search"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
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
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &SearchArgs| {
            format!("rg '{}' {}", a.query, a.path)
        })
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        let a: SearchArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(confirm(
            format!("Buscar '{}'. Aprova executar: {cmd}?", a.query),
            default_accept_for(&a.path),
        ))
    }

    async fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: SearchArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
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

    fn is_read_only(&self) -> bool {
        true
    }
}
