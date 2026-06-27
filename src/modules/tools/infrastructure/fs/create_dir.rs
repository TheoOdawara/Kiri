#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

#[cfg(unix)]
use crate::modules::tools::application::command_sandbox::NetworkPolicy;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema, simple_command,
};
use crate::modules::tools::infrastructure::args::{PathArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::sandbox::default_accept_for;
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
        let cmd = self.command_line(sandbox, call)?;
        let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(confirm(
            format!("Criar diretório. Aprova executar: {cmd}?"),
            default_accept_for(&a.path),
        ))
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
            match exec::run_argv(
                &[
                    OsStr::new("mkdir"),
                    OsStr::new("-p"),
                    resolution.target.as_os_str(),
                ],
                Some(&cwd),
                None,
                &[],
                exec::DEFAULT_TIMEOUT,
                sandbox.confiner(),
                &sandbox.command_policy(NetworkPolicy::Deny, &[], &[&cwd]),
            )
            .await
            {
                Ok(result) if result.succeeded() => {
                    ToolOutcome::Ok(format!("created directory {}", args.path))
                }
                Ok(result) => ToolOutcome::Error(format!(
                    "cannot create {}: {}",
                    args.path,
                    result.stderr_text()
                )),
                Err(error) => {
                    ToolOutcome::Error(format!("cannot create {}: {}", args.path, error.message()))
                }
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
