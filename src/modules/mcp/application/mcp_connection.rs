use serde_json::Value;

use crate::shared::kernel::error::AgentResult;

/// One tool an MCP server advertises, in a shape ready to become a `tools::application::tool::Tool`
/// schema (`function_schema(name, description, input_schema)`).
#[derive(Debug, Clone)]
pub struct McpToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Port: a live connection to one MCP server. Implemented once (`RmcpConnection`, over the official
/// `rmcp` SDK) — the sanctioned site for this context's process I/O. `Send`-bound: unlike the TUI engine
/// ports, nothing here holds a `!Send` value (like `&dyn Sandbox`) across an `.await`.
#[async_trait::async_trait]
pub trait McpConnection: Send + Sync {
    /// The tools this server currently advertises.
    async fn list_tools(&self) -> AgentResult<Vec<McpToolSpec>>;
    /// Call `name` with `arguments` (a JSON object), returning its result as plain text. An MCP-reported
    /// tool-level error still returns `Ok` (the text explains the failure, for the model to read and
    /// recover from, matching `ToolOutcome::Error`'s convention elsewhere) — only a transport/protocol
    /// failure is `Err`.
    async fn call_tool(&self, name: &str, arguments: Value) -> AgentResult<String>;
}
