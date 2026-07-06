use std::path::Path;

use serde_json::{Value, json};

use crate::modules::tools::application::path::default_accept_for;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, PATH_DESC, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{MoveArgs, parse, parse_args};
use crate::modules::tools::infrastructure::exec;
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
             (with user confirmation) and overwrites an existing destination (with confirmation). Each \
             path is relative to the workspace root, or an absolute / ~ path to reach outside it.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["source", "destination"],
                "properties": {
                    "source": { "type": "string", "description": PATH_DESC },
                    "destination": { "type": "string", "description": PATH_DESC }
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
        if let Err(out) = ensure_parent_dirs(&resolution, &args.destination).await {
            return out;
        }

        match tokio::time::timeout(
            exec::DEFAULT_TIMEOUT,
            rename_or_copy(&source, &resolution.target),
        )
        .await
        {
            Ok(Ok(())) => ToolOutcome::Ok(format!("moved {} to {}", args.source, args.destination)),
            Ok(Err(error)) => ToolOutcome::Error(format!(
                "cannot move {} to {}: {error}",
                args.source, args.destination
            )),
            // `tokio::fs` runs on the blocking pool and can't be cancelled once dispatched: the
            // rename/copy/remove may still land after this timeout is reported (issue #53,
            // security-debt).
            Err(_) => ToolOutcome::Error(format!(
                "cannot move {} to {}: timed out (it may still complete in the background)",
                args.source, args.destination
            )),
        }
    }
}

/// Move `source` to `target`, falling back to copy-then-remove when `rename` cannot move across
/// filesystems (`ErrorKind::CrossesDevices` — e.g. two different drives on Windows, or a bind-mounted
/// `/tmp` on Linux). Only a plain file gets the fallback: a directory's cross-device move would need a
/// recursive copy, and copying nested symlinks safely (without following one into an arbitrary target
/// outside the tree) is a feature of its own — until it exists, a cross-device directory move surfaces
/// this `CrossesDevices` error rather than silently mishandling a nested symlink. The caller bounds the
/// whole sequence by `DEFAULT_TIMEOUT`.
async fn rename_or_copy(source: &Path, target: &Path) -> std::io::Result<()> {
    match tokio::fs::rename(source, target).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            let is_file = tokio::fs::metadata(source)
                .await
                .map(|metadata| metadata.is_file())
                .unwrap_or(false);
            if !is_file {
                return Err(error);
            }
            tokio::fs::copy(source, target).await?;
            tokio::fs::remove_file(source).await
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::tool_call::FunctionCall;
    use std::fs;
    use tempfile::TempDir;

    fn call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "move_path".to_string(),
                arguments: args.to_string(),
            },
        }
    }

    // The cross-device (`CrossesDevices`) fallback in `rename_or_copy` needs two real filesystems/
    // devices to exercise, which a single `TempDir` cannot simulate portably — these tests cover the
    // same-device `rename` path `move_path` takes the overwhelming majority of the time.

    #[tokio::test]
    async fn move_path_renames_a_file_and_creates_missing_dest_dirs() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"content").unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = MovePath
            .execute(
                &sb,
                &call(json!({"source": "a.txt", "destination": "sub/b.txt"})),
            )
            .await;
        assert!(
            matches!(outcome, ToolOutcome::Ok(_)),
            "expected Ok, got {outcome:?}"
        );
        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("sub/b.txt")).unwrap(),
            "content"
        );
    }

    #[tokio::test]
    async fn move_path_moves_a_directory() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/f.txt"), b"x").unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = MovePath
            .execute(&sb, &call(json!({"source": "src", "destination": "dst"})))
            .await;
        assert!(matches!(outcome, ToolOutcome::Ok(_)));
        assert!(!dir.path().join("src").exists());
        assert!(dir.path().join("dst/f.txt").is_file());
    }

    #[tokio::test]
    async fn move_path_refuses_to_move_the_workspace_root() {
        let dir = TempDir::new().unwrap();
        let sb = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
        let outcome = MovePath
            .execute(
                &sb,
                &call(json!({"source": ".", "destination": "elsewhere"})),
            )
            .await;
        match outcome {
            ToolOutcome::Error(msg) => assert!(msg.contains("workspace root"), "got: {msg}"),
            other => panic!("expected refusal, got {other:?}"),
        }
    }
}
