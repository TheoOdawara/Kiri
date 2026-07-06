use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_command, simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{SearchArgs, parse, parse_args};
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::sandbox::SECRET_DIRS;
use crate::modules::tools::infrastructure::support::SEARCH_MAX_MATCHES;
use crate::modules::tools::infrastructure::support::search_file;
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

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &SearchArgs| {
            format!("rg '{}' {}", a.query, a.path)
        })
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: SearchArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation(
            &format!("Buscar '{}'", a.query),
            self.command_line(sandbox, call),
            &a.path,
        )
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: SearchArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        if args.query.is_empty() {
            return ToolOutcome::Error("query must not be empty".to_string());
        }
        // `resolve_existing` already refuses a start path inside a credential directory (`.ssh`/…), so
        // a search rooted at a secret dir is rejected here; the `--exclude-dir` flags below stop the
        // recursion from descending into a secret dir nested *under* an allowed start.
        let start = match sandbox.resolve_existing(&args.path) {
            Ok(start) => start,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        // Bound the recursion (and the displayed paths) to the resolved start directory, so a search
        // begun outside the active workspace stays within its own subtree. The whole walk is bounded
        // by `DEFAULT_TIMEOUT` (a wedged/stale network mount must fail fast, never stall the runtime).
        // `matches`/`truncated` are captured by mutable reference (not moved into the future) so that
        // whatever was found before a timeout survives the future's cancellation-on-drop.
        let base = start.clone();
        let mut matches: Vec<String> = Vec::new();
        let mut truncated = false;
        let timed_out = {
            let matches = &mut matches;
            let truncated = &mut truncated;
            let walk = async move {
                let mut stack = vec![start];
                'dirs: while let Some(dir) = stack.pop() {
                    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                        continue;
                    };
                    loop {
                        let entry = match entries.next_entry().await {
                            Ok(Some(entry)) => entry,
                            Ok(None) => break,
                            // Mirrors `list_dir`'s tolerance for a single bad entry (the pre-existing
                            // `entries.flatten()` behavior): skip it and keep scanning this directory.
                            Err(_) => continue,
                        };
                        let Ok(file_type) = entry.file_type().await else {
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
                            // Never descend into a credential directory, so files there with
                            // non-sensitive names (e.g. `.aws/config`) are not scanned.
                            if entry.file_name().to_str().is_some_and(|name| {
                                SECRET_DIRS.iter().any(|dir| dir.eq_ignore_ascii_case(name))
                            }) {
                                continue;
                            }
                            stack.push(path);
                        } else if file_type.is_file() {
                            if entry
                                .file_name()
                                .to_str()
                                .is_some_and(|name| sandbox.is_sensitive_name(name))
                            {
                                continue; // never leak the contents of a sensitive file
                            }
                            search_file(&path, &args.query, &base, matches).await;
                            if matches.len() >= SEARCH_MAX_MATCHES {
                                *truncated = true;
                                break 'dirs;
                            }
                        }
                    }
                }
            };
            tokio::time::timeout(exec::DEFAULT_TIMEOUT, walk)
                .await
                .is_err()
        };

        if matches.is_empty() {
            return ToolOutcome::Ok(if timed_out {
                "search timed out before any match was found".to_string()
            } else {
                "no matches".to_string()
            });
        }
        let mut output = matches.join("\n");
        if truncated {
            output.push_str(&format!("\n… (truncated at {SEARCH_MAX_MATCHES} matches)"));
        } else if timed_out {
            output.push_str("\n… (search timed out; results may be incomplete)");
        }
        ToolOutcome::Ok(output)
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Search;
    use crate::modules::tools::application::tool::{Tool, ToolOutcome};
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};
    use std::fs;
    use tempfile::TempDir;

    fn call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "search".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn search_does_not_descend_into_credential_dirs() {
        // Files with non-sensitive *names* (`config`, `config.json`) inside a credential *dir* must not
        // be reached by the recursion, even though the file-name guard would not catch them.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"TOKEN in code").unwrap();
        fs::create_dir(dir.path().join(".aws")).unwrap();
        fs::write(dir.path().join(".aws").join("config"), b"aws_secret=TOKEN").unwrap();
        fs::create_dir(dir.path().join(".docker")).unwrap();
        fs::write(
            dir.path().join(".docker").join("config.json"),
            b"{\"auth\":\"TOKEN\"}",
        )
        .unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();

        let outcome = Search
            .execute(&sb, &call(serde_json::json!({"query": "TOKEN"})))
            .await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(
                    text.contains("a.txt"),
                    "normal match must be present: {text}"
                );
                assert!(
                    !text.contains(".aws")
                        && !text.contains(".docker")
                        && !text.contains("aws_secret")
                        && !text.contains("auth"),
                    "a credential dir's files must not be searched: {text}"
                );
            }
            other => panic!("expected matches, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_refuses_a_secret_directory() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".ssh")).unwrap();
        fs::write(
            dir.path().join(".ssh").join("id_rsa"),
            b"PRIVATE KEY material",
        )
        .unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();

        let outcome = Search
            .execute(
                &sb,
                &call(serde_json::json!({"query": "PRIVATE", "path": ".ssh"})),
            )
            .await;
        match outcome {
            ToolOutcome::Error(msg) => assert!(
                msg.contains("secret directory"),
                "expected a secret-dir refusal, got: {msg}"
            ),
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_drops_matches_from_sensitive_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"NEEDLE in code").unwrap();
        fs::write(dir.path().join(".env"), b"SECRET=NEEDLE").unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::new(&[".env"]).unwrap()).unwrap();

        let outcome = Search
            .execute(&sb, &call(serde_json::json!({"query": "NEEDLE"})))
            .await;
        match outcome {
            ToolOutcome::Ok(text) => {
                assert!(
                    text.contains("a.txt"),
                    "normal match must be present: {text}"
                );
                assert!(
                    !text.contains(".env") && !text.contains("SECRET"),
                    "a sensitive file's contents must be filtered out: {text}"
                );
            }
            other => panic!("expected matches, got {other:?}"),
        }
    }
}
