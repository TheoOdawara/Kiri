use serde_json::Value;

use crate::shared::kernel::error::AgentResult;

/// One tool an MCP server advertises, shaped for `tools::application::tool::function_schema`.
#[derive(Debug, Clone)]
pub struct McpToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A live connection to one MCP server. `Send`-bound, unlike the TUI engine ports: nothing here holds a
/// `!Send` value across an `.await`.
#[async_trait::async_trait]
pub trait McpConnection: Send + Sync {
    async fn list_tools(&self) -> AgentResult<Vec<McpToolSpec>>;
    /// An MCP-reported tool-level error still returns `Ok`, carrying the text for the model to recover
    /// from (matching `ToolOutcome::Error`); only a transport/protocol failure is `Err`.
    async fn call_tool(&self, name: &str, arguments: Value) -> AgentResult<String>;
}
