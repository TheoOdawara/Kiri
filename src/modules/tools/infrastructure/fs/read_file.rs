#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, confirm, function_schema, simple_command,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::sandbox::default_accept_for;
use crate::modules::tools::infrastructure::support::READ_FILE_MAX_BYTES;
#[cfg(windows)]
use crate::modules::tools::infrastructure::support::read_capped;
#[cfg(unix)]
use crate::shared::kernel::sandbox::NetworkPolicy;
use crate::shared::kernel::tool_call::ToolCall;

pub struct ReadFile;

#[async_trait::async_trait(?Send)]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Read a UTF-8 text file and return its contents. Paths are relative to the active workspace \
             root; an absolute path or '~/…' may reach outside it (the user confirms each call). '..' \
             in a relative path is rejected.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": PATH_DESC } }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("cat {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(confirm(
            format!("Ler o arquivo. Aprova executar: {cmd}?"),
            default_accept_for(&a.path),
        ))
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: PathArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let path = match sandbox.resolve_existing(&args.path) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };

        // `head -c (cap+1)` bounds the read just like the native `read_capped`; the truncation marker
        // below is still decided here, so the model sees the exact same output as before.
        #[cfg(unix)]
        let bytes = {
            let cap = (READ_FILE_MAX_BYTES + 1).to_string();
            let cwd = sandbox.exec_cwd_for(&path);
            let result = match exec::run_argv(
                &[
                    OsStr::new("head"),
                    OsStr::new("-c"),
                    OsStr::new(&cap),
                    path.as_os_str(),
                ],
                Some(&cwd),
                None,
                &[],
                exec::DEFAULT_TIMEOUT,
                sandbox.confiner(),
                // Read-only: pass no extras. The cwd read-allow is redundant under the macOS
                // `(allow default)` base and, emitted last, would override the home-credential denies
                // when the workspace root is a home ancestor (TOOL-07) — a least-privilege regression.
                &sandbox.command_policy(NetworkPolicy::Deny, &[], &[]),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    return ToolOutcome::Error(format!(
                        "cannot read {}: {}",
                        args.path,
                        error.message()
                    ));
                }
            };
            if !result.succeeded() {
                return ToolOutcome::Error(format!(
                    "cannot read {}: {}",
                    args.path,
                    result.stderr_text()
                ));
            }
            result.stdout
        };

        #[cfg(windows)]
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

    fn is_read_only(&self) -> bool {
        true
    }
}
