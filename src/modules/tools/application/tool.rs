use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::infrastructure::args::parse;
use crate::modules::tools::infrastructure::sandbox::default_accept_for;
use crate::shared::kernel::tool_call::ToolCall;

/// The result of executing a tool. Failures are data the model reads and recovers from — never panics
/// nor `Err` that would abort the agentic turn.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Ok(String),
    Error(String),
    Declined,
}

impl ToolOutcome {
    /// The content placed in the `role: tool` message returned to the model.
    pub fn into_message_content(self) -> String {
        match self {
            ToolOutcome::Ok(text) => text,
            ToolOutcome::Error(error) => format!("error: {error}"),
            ToolOutcome::Declined => "declined by user".to_string(),
        }
    }
}

/// A confirmation request: the line to show and whether Enter approves it. Operations inside the active
/// workspace default to accept (`[S/n]`); operations on an explicit absolute/`~` path (potentially
/// outside the workspace) default to decline (`[s/N]`), requiring a deliberate "yes".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub prompt: String,
    pub default_accept: bool,
}

/// Build a confirmation line from a phrased action and its default-accept flag, appending the `[S/n]`
/// or `[s/N]` suffix. Shared by every tool so the suffix rule lives in one place.
pub fn confirm(action: String, default_accept: bool) -> Confirmation {
    let suffix = if default_accept { "[S/n]" } else { "[s/N]" };
    Confirmation {
        prompt: format!("{action} {suffix} "),
        default_accept,
    }
}

/// Build the standard single-path confirmation: `{phrase}. Aprova executar: {cmd}?`, with the
/// default-accept derived from whether `path` reaches outside the workspace (relative → accept
/// `[S/n]`, explicit absolute/`~` → decline `[s/N]`). `None` when `command_line` is `None` (the args
/// did not parse, so `execute` surfaces the parse error). Single-sources the prompt skeleton and the
/// in/out-of-workspace default rule shared by the read/edit/list/create/delete/search confirmations.
pub fn simple_path_confirmation(
    phrase: &str,
    command_line: Option<String>,
    path: &str,
) -> Option<Confirmation> {
    let cmd = command_line?;
    Some(confirm(
        format!("{phrase}. Aprova executar: {cmd}?"),
        default_accept_for(path),
    ))
}

/// Build the bare command label for a tool whose label is a fixed render over its parsed args.
/// Returns `None` when the args do not parse. The single source of a tool's command text, reused by
/// both `Tool::command_line` (for on-screen display) and `Tool::confirmation` (for the prompt prose).
pub fn simple_command<T: DeserializeOwned>(
    call: &ToolCall,
    render: impl FnOnce(&T) -> String,
) -> Option<String> {
    let args: T = parse(call.function.arguments.as_str()).ok()?;
    Some(render(&args))
}

/// The `path` property description shared verbatim by the tools that take a single
/// workspace-relative-or-absolute path. Hoisted so the four byte-identical schemas have one source;
/// the characterization snapshot pins the exact text.
pub const PATH_DESC: &str =
    "Path relative to the active workspace root, or an absolute / ~ path to reach outside it.";

/// The full advertised tool object (the OpenAI-compatible `{type, function:{…}}` shape) a tool puts on
/// the wire. Shared so every `Tool::schema` is built the same way.
pub fn function_schema(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": { "name": name, "description": description, "parameters": parameters }
    })
}

/// A self-describing file tool: its wire schema, its pt-BR confirmation phrasing, and its execution
/// against the sandbox. Adding a tool is one new file implementing this trait, registered in
/// `infrastructure::fs::default_fs_tools`. The `execute` method is async so a tool can await
/// external processes (e.g. `run_command` awaiting a child) without blocking the runtime; the
/// `?Send` flavor matches the single-threaded TUI runtime (the rest of the engine already uses it).
#[async_trait::async_trait(?Send)]
pub trait Tool: Send + Sync {
    /// The stable name the model calls (e.g. `"read_file"`).
    fn name(&self) -> &'static str;
    /// The full tool object advertised to the model.
    fn schema(&self) -> Value;
    /// The bare command this call represents, for on-screen display (e.g. `edit src/x.rs`, `cat foo`,
    /// `rg 'q' .`). `None` only when the args do not parse. `confirmation` composes its prose around
    /// this, so the command text lives in one place.
    fn command_line(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String>;
    /// Phrase the confirmation from the parsed args; `None` only when the args do not parse (then
    /// `execute` reports the error). May resolve paths via the sandbox to phrase write/move precisely.
    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation>;
    /// Run the call against the sandbox. Never panics nor returns `Err` that aborts the turn. Async so
    /// tools that spawn processes (`run_command`) can await them; the fast file tools keep their
    /// blocking `std::fs` bodies — they complete in microseconds and the runtime is unaffected.
    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome;
    /// Whether the tool only reads, never mutating the filesystem. Read-only tools stay available in
    /// plan mode and run without confirmation while planning. Defaults to `false` (treated as
    /// destructive), so a new tool is gated unless it explicitly opts in.
    fn is_read_only(&self) -> bool {
        false
    }
    /// Whether the tool is advertised in plan mode. Defaults to `is_read_only` — read-only tools are
    /// always plannable. A tool that can mutate but is safe to run for investigation (e.g.
    /// `run_command` for starting a dev server or reading logs) overrides this to `true`; the
    /// plan-mode blacklist (in `run_command::execute`) handles the destructive cases.
    fn is_plannable(&self) -> bool {
        self.is_read_only()
    }
    /// Whether the tool is advertised *only* in plan mode, never in default/auto. Defaults to `false`.
    /// A plan-only tool (e.g. `present_plan`) is a planning control surface, not a filesystem action,
    /// so it must not appear outside the plan workflow. `schemas()` excludes it; `schemas_for(Plan)`
    /// keeps it via `is_plannable`.
    fn plan_only(&self) -> bool {
        false
    }
    /// In plan mode, check whether this call should be blocked before execution. Returns
    /// `Some(reason)` if blocked, `None` if allowed. Defaults to `None` — tools that need
    /// plan-mode restrictions (e.g. `run_command` checking a command blacklist) override this.
    fn plan_check(&self, _sandbox: &dyn Sandbox, _call: &ToolCall) -> Option<String> {
        None
    }
    /// Whether this tool must still be confirmed in auto mode — a high-blast-radius / irreversible
    /// action. Defaults to `false`: ordinary mutations (write/edit/create_dir) run unattended in
    /// auto, while the engine independently gates any out-of-root target. Overridden to `true` by the
    /// irreversible tools (`run_command`, `delete_file`, `delete_dir`, `move_path`) so an unattended
    /// turn — including a prompt-injected one — can never silently destroy data or run a shell.
    fn confirm_in_auto(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_into_message_content_maps_variants() {
        assert_eq!(
            ToolOutcome::Ok("ok".to_string()).into_message_content(),
            "ok"
        );
        assert_eq!(
            ToolOutcome::Error("boom".to_string()).into_message_content(),
            "error: boom"
        );
        assert_eq!(
            ToolOutcome::Declined.into_message_content(),
            "declined by user"
        );
    }

    #[test]
    fn simple_path_confirmation_default_accepts_relative_only() {
        // A workspace-relative path defaults to accept ([S/n]); an explicit absolute path
        // (potentially outside the workspace) defaults to decline ([s/N]). Locks the in/out-of-
        // workspace default rule the single-path confirmations all funnel through.
        let relative =
            simple_path_confirmation("Ler o arquivo", Some("cat a.txt".to_string()), "a.txt")
                .expect("a parsed command yields a confirmation");
        assert!(relative.default_accept);
        assert!(relative.prompt.contains("cat a.txt"));

        let absolute = simple_path_confirmation(
            "Ler o arquivo",
            Some("cat /etc/hosts".to_string()),
            "/etc/hosts",
        )
        .expect("a parsed command yields a confirmation");
        assert!(!absolute.default_accept);

        // No command line (the args did not parse) yields no confirmation.
        assert!(simple_path_confirmation("Ler o arquivo", None, "a.txt").is_none());
    }
}
