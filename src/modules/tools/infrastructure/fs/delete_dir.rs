#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

#[cfg(unix)]
use crate::modules::tools::application::command_sandbox::NetworkPolicy;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema, simple_command,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::sandbox::{Sandbox, default_accept_for};
use crate::modules::tools::infrastructure::support::stat_guard;
use crate::shared::kernel::tool_call::ToolCall;

pub struct DeleteDir;

#[async_trait::async_trait(?Send)]
impl Tool for DeleteDir {
    fn name(&self) -> &'static str {
        "delete_dir"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Delete a directory and all of its contents, recursively. Requires confirmation. The path \
             is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Directory path to delete, relative to the workspace root." } }
            }),
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("rm -rf {}", a.path))
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(confirm(
            format!(
                "Excluir recursivamente o diretório e todo o seu conteúdo. Aprova executar: {cmd}?"
            ),
            default_accept_for(&a.path),
        ))
    }

    fn confirm_in_auto(&self) -> bool {
        true
    }

    async fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: PathArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let path = match sandbox.resolve_existing(&args.path) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if path == sandbox.root() {
            return ToolOutcome::Error("refusing to delete the workspace root".to_string());
        }
        if let Err(out) = stat_guard(&path, &args.path, |metadata| {
            (!metadata.is_dir())
                .then(|| format!("{} is not a directory; use delete_file", args.path))
        }) {
            return out;
        }

        #[cfg(unix)]
        {
            let cwd = sandbox.exec_cwd_for(&path);
            match exec::run_argv(
                &[OsStr::new("rm"), OsStr::new("-rf"), path.as_os_str()],
                Some(&cwd),
                None,
                &[],
                exec::DEFAULT_TIMEOUT,
                sandbox.confiner(),
                &sandbox.command_policy(NetworkPolicy::Deny, &[&cwd]),
            )
            .await
            {
                Ok(result) if result.succeeded() => {
                    ToolOutcome::Ok(format!("deleted directory {}", args.path))
                }
                Ok(result) => ToolOutcome::Error(format!(
                    "cannot delete {}: {}",
                    args.path,
                    result.stderr_text()
                )),
                Err(error) => {
                    ToolOutcome::Error(format!("cannot delete {}: {}", args.path, error.message()))
                }
            }
        }

        #[cfg(windows)]
        {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => ToolOutcome::Ok(format!("deleted directory {}", args.path)),
                Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
            }
        }
    }
}
