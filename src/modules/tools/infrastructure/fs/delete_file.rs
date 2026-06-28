#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, function_schema, simple_command,
    simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::support::run_fs_argv;
use crate::modules::tools::infrastructure::support::stat_guard;
use crate::shared::kernel::tool_call::ToolCall;

pub struct DeleteFile;

#[async_trait::async_trait(?Send)]
impl Tool for DeleteFile {
    fn name(&self) -> &'static str {
        "delete_file"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Delete a file (not a directory). The path is relative to the workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": PATH_DESC } }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("rm {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation(
            "Excluir o arquivo",
            self.command_line(sandbox, call),
            &a.path,
        )
    }

    fn confirm_in_auto(&self) -> bool {
        true
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
        if let Err(out) = stat_guard(&path, &args.path, |metadata| {
            metadata
                .is_dir()
                .then(|| format!("{} is a directory; not deleted", args.path))
        })
        .await
        {
            return out;
        }

        #[cfg(unix)]
        {
            let cwd = sandbox.exec_cwd_for(&path);
            match run_fs_argv(
                sandbox,
                &[OsStr::new("rm"), path.as_os_str()],
                &cwd,
                None,
                &[],
                &[&cwd],
                &format!("delete {}", args.path),
            )
            .await
            {
                Ok(_) => ToolOutcome::Ok(format!("deleted {}", args.path)),
                Err(out) => out,
            }
        }

        #[cfg(windows)]
        {
            match std::fs::remove_file(&path) {
                Ok(()) => ToolOutcome::Ok(format!("deleted {}", args.path)),
                Err(error) => ToolOutcome::Error(format!("cannot delete {}: {error}", args.path)),
            }
        }
    }
}
