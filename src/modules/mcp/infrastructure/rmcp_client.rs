//! The sanctioned site for the `mcp` context's process/network I/O (ADR 0021): spawns an MCP server as a
//! child process (stdio transport) via the official `rmcp` SDK and speaks the protocol over it. One
//! adapter, `RmcpConnection`, kept alive for the session's lifetime once connected.

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde_json::Value;
use tokio::process::Command;

use crate::modules::mcp::application::mcp_connection::{McpConnection, McpToolSpec};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::error::AgentResult;

/// A live connection to one MCP server, spawned as a child process over stdio. `Send`-bound (rmcp's
/// service handle is `Send + Sync`), unlike the engine's `?Send` ports — this context has no `&dyn
/// Sandbox`/`!Send` value crossing an await point.
pub struct RmcpConnection {
    service: RunningService<RoleClient, ()>,
}

impl RmcpConnection {
    /// Spawn `command args...` and complete the MCP handshake over its stdio. A spawn failure or a
    /// handshake failure both surface as `AgentError::Extensions` — the caller (`app::wire`) treats a
    /// server that fails to connect like any other auxiliary degradation: a boot notice, never fatal.
    pub async fn connect(command: &str, args: &[String]) -> AgentResult<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        let transport = TokioChildProcess::new(cmd)
            .map_err(|error| AgentError::extensions(format!("spawn '{command}': {error}")))?;
        let service = ()
            .serve(transport)
            .await
            .map_err(|error| AgentError::extensions(format!("MCP handshake failed: {error}")))?;
        Ok(Self { service })
    }
}

#[async_trait::async_trait]
impl McpConnection for RmcpConnection {
    async fn list_tools(&self) -> AgentResult<Vec<McpToolSpec>> {
        let result = self
            .service
            .peer()
            .list_tools(Default::default())
            .await
            .map_err(|error| AgentError::extensions(format!("list_tools failed: {error}")))?;
        Ok(result
            .tools
            .into_iter()
            .map(|tool| McpToolSpec {
                name: tool.name.to_string(),
                description: tool.description.map(|d| d.to_string()).unwrap_or_default(),
                input_schema: Value::Object((*tool.input_schema).clone()),
            })
            .collect())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> AgentResult<String> {
        let mut params = CallToolRequestParams::new(name.to_string());
        if let Some(object) = arguments.as_object() {
            params = params.with_arguments(object.clone());
        }
        let result = self
            .service
            .peer()
            .call_tool(params)
            .await
            .map_err(|error| {
                AgentError::extensions(format!("call_tool '{name}' failed: {error}"))
            })?;
        let text = result
            .content
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text_content) => Some(text_content.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if result.is_error == Some(true) {
            Ok(format!("MCP tool '{name}' reported an error: {text}"))
        } else {
            Ok(text)
        }
    }
}
