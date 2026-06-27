#[cfg(unix)]
use std::ffi::OsStr;

use serde_json::{Value, json};

#[cfg(unix)]
use crate::modules::tools::application::command_sandbox::NetworkPolicy;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{MoveArgs, parse, parse_args};
#[cfg(unix)]
use crate::modules::tools::infrastructure::exec;
use crate::modules::tools::infrastructure::sandbox::default_accept_for;
use crate::modules::tools::infrastructure::support::{ensure_parent_dirs, missing_dirs_label};
use crate::shared::kernel::tool_call::ToolCall;

pub struct MovePath;

#[async_trait::async_trait(?Send)]
impl Tool for MovePath {
    fn name(&self) -> &'static str {
        "move_path"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
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
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let a: MoveArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("mv {} {}", a.source, a.destination))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let a: MoveArgs = parse(call.function.arguments.as_str()).ok()?;
        let cmd = self.command_line(sandbox, call)?;
        let action = match sandbox.resolve_create(&a.destination) {
            Ok(r) if !r.missing_dirs.is_empty() => format!(
                "Criar diretório(s) '{}' e mover. Aprova executar: {cmd}?",
                missing_dirs_label(&r, sandbox),
            ),
            Ok(r) if r.target.exists() => {
                format!("Sobrescrever o destino movendo. Aprova executar: {cmd}?")
            }
            // Also covers a resolve_create error: the user is still asked (returning None here would
            // skip confirmation), and execute() re-validates the path and surfaces the real error.
            _ => format!("Mover o caminho. Aprova executar: {cmd}?"),
        };
        let default_accept = default_accept_for(&a.destination);
        Some(confirm(action, default_accept))
    }

    fn confirm_in_auto(&self) -> bool {
        true
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: MoveArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
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
        if let Err(out) = ensure_parent_dirs(&resolution, &args.destination) {
            return out;
        }

        #[cfg(unix)]
        {
            let cwd = sandbox.exec_cwd_for(&resolution.target);
            // A move writes at both ends: it creates the destination and unlinks the source. When
            // either is an approved out-of-root target, its directory must be in the write allow-list.
            let source_cwd = sandbox.exec_cwd_for(&source);
            match exec::run_argv(
                &[
                    OsStr::new("mv"),
                    source.as_os_str(),
                    resolution.target.as_os_str(),
                ],
                Some(&cwd),
                None,
                &[],
                exec::DEFAULT_TIMEOUT,
                sandbox.confiner(),
                &sandbox.command_policy(NetworkPolicy::Deny, &[], &[&cwd, &source_cwd]),
            )
            .await
            {
                Ok(result) if result.succeeded() => {
                    ToolOutcome::Ok(format!("moved {} to {}", args.source, args.destination))
                }
                Ok(result) => ToolOutcome::Error(format!(
                    "cannot move {} to {}: {}",
                    args.source,
                    args.destination,
                    result.stderr_text()
                )),
                Err(error) => ToolOutcome::Error(format!(
                    "cannot move {} to {}: {}",
                    args.source,
                    args.destination,
                    error.message()
                )),
            }
        }

        #[cfg(windows)]
        {
            match std::fs::rename(&source, &resolution.target) {
                Ok(()) => ToolOutcome::Ok(format!("moved {} to {}", args.source, args.destination)),
                Err(error) => ToolOutcome::Error(format!(
                    "cannot move {} to {}: {error}",
                    args.source, args.destination
                )),
            }
        }
    }
}
