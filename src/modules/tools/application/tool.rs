use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::modules::tools::infrastructure::args::parse;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
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
/// `infrastructure::fs::default_fs_tools`.
pub trait Tool: Send + Sync {
    /// The stable name the model calls (e.g. `"read_file"`).
    fn name(&self) -> &'static str;
    /// The full tool object advertised to the model.
    fn schema(&self) -> Value;
    /// The bare command this call represents, for on-screen display (e.g. `edit src/x.rs`, `cat foo`,
    /// `rg 'q' .`). `None` only when the args do not parse. `confirmation` composes its prose around
    /// this, so the command text lives in one place.
    fn command_line(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<String>;
    /// Phrase the confirmation from the parsed args; `None` only when the args do not parse (then
    /// `execute` reports the error). May resolve paths via the sandbox to phrase write/move precisely.
    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation>;
    /// Run the call against the sandbox. Never panics nor returns `Err` that aborts the turn.
    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome;
    /// Whether the tool only reads, never mutating the filesystem. Read-only tools stay available in
    /// plan mode and run without confirmation while planning. Defaults to `false` (treated as
    /// destructive), so a new tool is gated unless it explicitly opts in.
    fn is_read_only(&self) -> bool {
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
}
