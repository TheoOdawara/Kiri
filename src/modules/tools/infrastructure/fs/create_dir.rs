#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, function_schema, simple_command, simple_path_confirmation,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::support::run_fs_argv;
use crate::shared::kernel::tool_call::ToolCall;

pub struct CreateDir;

#[async_trait::async_trait(?Send)]
impl Tool for CreateDir {
    fn name(&self) -> &'static str {
        "create_dir"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Create a directory, including any missing parent directories. The path is relative to the \
             workspace root.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Directory path to create, relative to the workspace root." } }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        simple_command(call, |a: &PathArgs| format!("mkdir {}", a.path))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        simple_path_confirmation("Criar diretório", self.command_line(sandbox, call), &a.path)
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: PathArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let resolution = match sandbox.resolve_create(&args.path) {
            Ok(resolution) => resolution,
            Err(error) => return ToolOutcome::Error(error.to_string()),
        };
        if resolution.target.is_dir() {
            return ToolOutcome::Ok(format!("directory already exists: {}", args.path));
        }

        #[cfg(unix)]
        {
            let cwd = sandbox.exec_cwd_for(&resolution.target);
            match run_fs_argv(
                sandbox,
                &[
                    OsStr::new("mkdir"),
                    OsStr::new("-p"),
                    resolution.target.as_os_str(),
                ],
                &cwd,
                None,
                &[],
                &[&cwd],
                &format!("create {}", args.path),
            )
            .await
            {
                Ok(_) => ToolOutcome::Ok(format!("created directory {}", args.path)),
                Err(out) => out,
            }
        }

        #[cfg(windows)]
        {
            match std::fs::create_dir_all(&resolution.target) {
                Ok(()) => ToolOutcome::Ok(format!("created directory {}", args.path)),
                Err(error) => ToolOutcome::Error(format!("cannot create {}: {error}", args.path)),
            }
        }
    }
}
