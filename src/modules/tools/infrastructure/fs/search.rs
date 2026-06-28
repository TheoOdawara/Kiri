#[cfg(unix)]
use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::fs;

use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_command, simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{SearchArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::exec;
#[cfg(any(unix, windows))]
use crate::modules::tools::infrastructure::sandbox::SECRET_DIRS;
#[cfg(unix)]
use crate::modules::tools::infrastructure::support::SEARCH_MAX_LINE_CHARS;
use crate::modules::tools::infrastructure::support::SEARCH_MAX_MATCHES;
#[cfg(windows)]
use crate::modules::tools::infrastructure::support::search_file;
#[cfg(unix)]
use crate::shared::kernel::sandbox::NetworkPolicy;
use crate::shared::kernel::tool_call::ToolCall;

pub struct Search;

/// Reformat one `grep -rIFn` line (`./path:line:content`) to the native shape `path:line: content`:
/// drop the `./` grep prepends, and bound the content to the per-line char cap. (Parsing splits on the
/// first two `:` — `-Z`/`--null` would disambiguate a `:` in a filename, but its meaning differs across
/// grep implementations on PATH, e.g. ugrep treats `-Z` as fuzzy-match, so it is not relied upon.)
#[cfg(unix)]
fn format_grep_line(line: &str) -> String {
    let mut parts = line.splitn(3, ':');
    let path = parts.next().unwrap_or_default();
    let number = parts.next().unwrap_or_default();
    let content = parts.next().unwrap_or_default();
    let path = path.strip_prefix("./").unwrap_or(path);
    let shown: String = content.chars().take(SEARCH_MAX_LINE_CHARS).collect();
    format!("{path}:{number}: {shown}")
}

/// Whether a raw `grep` match line comes from a sensitive file (by its last path component). Such
/// matches are dropped so `search` cannot leak the contents of a `.env`/`id_rsa` the scan reached.
#[cfg(unix)]
fn is_sensitive_match(sandbox: &dyn Sandbox, grep_line: &str) -> bool {
    let path = grep_line.split(':').next().unwrap_or_default();
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| sandbox.is_sensitive_name(name))
}

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

        // `grep -rIFn` does the recursive scan: `-r` recurse (without following symlinked dirs), `-I`
        // skip binary files, `-F` fixed-string (literal, case-sensitive) match, `-n` line numbers.
        // `--exclude-dir` (supported by GNU/BSD/ugrep alike) keeps the scan out of credential
        // directories whose files (e.g. `.aws/config`) the file-name guard misses. The query is its own
        // argv element, so it is never shell-interpreted. The command runs *in* the resolved start
        // directory and searches `.`, so the paths grep prints are relative to it.
        #[cfg(unix)]
        {
            let mut argv: Vec<OsString> = vec![OsString::from("grep"), OsString::from("-rIFn")];
            for dir in SECRET_DIRS {
                argv.push(OsString::from(format!("--exclude-dir={dir}")));
            }
            argv.push(OsString::from("-e"));
            argv.push(OsString::from(args.query.as_str()));
            argv.push(OsString::from("."));
            let argv: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();
            let result = match exec::run_argv(
                &argv,
                Some(&start),
                None,
                &[],
                exec::DEFAULT_TIMEOUT,
                sandbox.confiner(),
                // Read-only: pass no extras. The start-dir read-allow is redundant under the macOS
                // `(allow default)` base and, emitted last, would override the home-credential denies
                // when the workspace root is a home ancestor (TOOL-07) — a least-privilege regression.
                &sandbox.command_policy(NetworkPolicy::Deny, &[], &[]),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    return ToolOutcome::Error(format!(
                        "cannot search {}: {}",
                        args.path,
                        error.message()
                    ));
                }
            };
            // grep exit status: 0 = matches found, 1 = none, >= 2 = a real error.
            if result.exit_code.unwrap_or(2) >= 2 {
                return ToolOutcome::Error(format!(
                    "cannot search {}: {}",
                    args.path,
                    result.stderr_text()
                ));
            }

            let stdout = String::from_utf8_lossy(&result.stdout);
            let mut matches: Vec<String> = Vec::new();
            let mut truncated = false;
            for line in stdout.lines() {
                if matches.len() >= SEARCH_MAX_MATCHES {
                    truncated = true;
                    break;
                }
                if is_sensitive_match(sandbox, line) {
                    continue;
                }
                matches.push(format_grep_line(line));
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

        #[cfg(windows)]
        {
            let mut matches: Vec<String> = Vec::new();
            // Bound the recursion (and the displayed paths) to the resolved start directory, so a search
            // begun outside the active workspace stays within its own subtree.
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
                        // Never descend into a credential directory (matches the Unix `--exclude-dir`),
                        // so files there with non-sensitive names (e.g. `.aws\config`) are not scanned.
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
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{SEARCH_MAX_LINE_CHARS, Search, format_grep_line};
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

    #[test]
    fn format_grep_line_strips_dot_slash_and_inserts_the_space() {
        assert_eq!(
            format_grep_line("./sub/f.txt:2:NEEDLE here"),
            "sub/f.txt:2: NEEDLE here"
        );
    }

    #[test]
    fn format_grep_line_truncates_long_content_at_a_char_boundary() {
        let long = "é".repeat(300);
        let shown = format_grep_line(&format!("f.txt:1:{long}"));
        let content = shown.rsplit_once(": ").unwrap().1;
        assert_eq!(content.chars().count(), SEARCH_MAX_LINE_CHARS);
        assert!(content.chars().all(|c| c == 'é'));
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
