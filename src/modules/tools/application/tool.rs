use serde_json::{Value, json};

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
    /// Phrase the confirmation from the parsed args; `None` only when the args do not parse (then
    /// `execute` reports the error). May resolve paths via the sandbox to phrase write/move precisely.
    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation>;
    /// Run the call against the sandbox. Never panics nor returns `Err` that aborts the turn.
    fn execute(&self, sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome;
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
