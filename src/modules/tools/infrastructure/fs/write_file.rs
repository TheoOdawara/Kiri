#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{PathArgs, WriteArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::sandbox::{Sandbox, default_accept_for};
use crate::modules::tools::infrastructure::support::{ensure_parent_dirs, missing_dirs_label};
use crate::shared::kernel::tool_call::ToolCall;

pub struct WriteFile;

#[async_trait::async_trait(?Send)]
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Create a file or overwrite it with the full given content. Creates missing parent \
             directories (with user confirmation). The path is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string", "description": PATH_DESC },
                    "content": { "type": "string", "description": "Full file content to write." }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("write {}", a.path))
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        let cmd = self.command_line(sandbox, call)?;
        let action = match sandbox.resolve_create(&a.path) {
            Ok(r) if !r.missing_dirs.is_empty() => format!(
                "Criar diretório(s) '{}' e gravar o arquivo. Aprova executar: {cmd}?",
                missing_dirs_label(&r, sandbox),
            ),
            Ok(r) if r.target.exists() => {
                format!("Sobrescrever o arquivo. Aprova executar: {cmd}?")
            }
            _ => format!("Criar e gravar o arquivo. Aprova executar: {cmd}?"),
        };
        let default_accept = default_accept_for(&a.path);
        Some(confirm(action, default_accept))
    }

    async fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: WriteArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let resolution = match sandbox.resolve_create(&args.path) {
            Ok(resolution) => resolution,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if let Err(out) = ensure_parent_dirs(&resolution, &args.path) {
            return out;
        }

        // Pipe the content through the child's stdin so arbitrary bytes (newlines, quotes, `$`) reach
        // `tee` untouched — nothing is interpolated into the command. `tee` truncates/creates the file.
        #[cfg(unix)]
        {
            let cwd = sandbox.exec_cwd_for(&resolution.target);
            match exec::run_argv(
                &[OsStr::new("tee"), resolution.target.as_os_str()],
                Some(&cwd),
                Some(args.content.as_bytes()),
                &[],
                exec::DEFAULT_TIMEOUT,
            )
            .await
            {
                Ok(result) if result.succeeded() => ToolOutcome::Ok(format!(
                    "wrote {} bytes to {}",
                    args.content.len(),
                    args.path
                )),
                Ok(result) => ToolOutcome::Error(format!(
                    "cannot write {}: {}",
                    args.path,
                    result.stderr_text()
                )),
                Err(error) => {
                    ToolOutcome::Error(format!("cannot write {}: {}", args.path, error.message()))
                }
            }
        }

        #[cfg(windows)]
        {
            match std::fs::write(&resolution.target, args.content.as_bytes()) {
                Ok(()) => ToolOutcome::Ok(format!(
                    "wrote {} bytes to {}",
                    args.content.len(),
                    args.path
                )),
                Err(error) => ToolOutcome::Error(format!("cannot write {}: {error}", args.path)),
            }
        }
    }
}
