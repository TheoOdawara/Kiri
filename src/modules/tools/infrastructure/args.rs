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

#[derive(Deserialize)]
pub struct RunCommandArgs {
    pub command: String,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_path() -> String {
    ".".to_string()
}

fn default_cwd() -> String {
    ".".to_string()
}

/// The default `run_command` timeout. The single source the serde default, the JSON schema's `default`,
/// the tool description, and the system prompt all read — so the advertised default cannot drift from the
/// enforced one (SEC-06). Placed here, the lowest-level tool-args module, so `run_command.rs` (which
/// already depends on `args.rs`) reaches it without a module cycle.
pub const RUN_COMMAND_DEFAULT_TIMEOUT_MS: u64 = 30_000;

fn default_timeout_ms() -> u64 {
    RUN_COMMAND_DEFAULT_TIMEOUT_MS
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_ms_equals_the_const() {
        assert_eq!(default_timeout_ms(), RUN_COMMAND_DEFAULT_TIMEOUT_MS);
    }
}
