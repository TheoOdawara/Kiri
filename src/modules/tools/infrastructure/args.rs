use serde::Deserialize;

use crate::modules::tools::application::tool::ToolOutcome;
use crate::shared::kernel::tool_call::ToolCall;

#[derive(Deserialize)]
pub struct PathArgs {
    pub path: String,
}

#[derive(Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct MoveArgs {
    pub source: String,
    pub destination: String,
}

#[derive(Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
}

#[derive(Deserialize)]
pub struct ListArgs {
    #[serde(default = "default_path")]
    pub path: String,
}

#[derive(Deserialize)]
pub struct SearchArgs {
    pub query: String,
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_path() -> String {
    ".".to_string()
}

pub fn parse<T: serde::de::DeserializeOwned>(args: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(args)
}

/// Parse a tool call's JSON arguments, mapping a parse failure to the shared `invalid arguments`
/// outcome the model reads and recovers from — the prologue every tool's `execute` opens with.
pub fn parse_args<T: serde::de::DeserializeOwned>(call: &ToolCall) -> Result<T, ToolOutcome> {
    parse(call.function.arguments.as_str())
        .map_err(|error| ToolOutcome::Error(format!("invalid arguments: {error}")))
}
