//! The sanctioned site for the `mcp` context's process/network I/O (ADR 0021). Stdio transport only;
//! the connection is kept alive for the session's lifetime once established.

use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;

use crate::modules::mcp::application::mcp_connection::{McpConnection, McpToolSpec};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::error::AgentResult;

/// A hung or malicious server must not stall `app::wire` forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// A hung remote call must not strand a turn's busy state.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The allow-list re-added after `env_clear()`: `tokio::process::Command` inherits the *entire* parent env
/// by default, which would leak provider API keys and sandbox flags into an approved project-layer
/// server's process. ADR 0021 promises a server never receives a harness secret.
const INHERITED_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "SystemRoot",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
];

pub struct RmcpConnection {
    service: RunningService<RoleClient, ()>,
}

impl RmcpConnection {
    /// Every failure path surfaces as `AgentError::Extensions`; `app::wire` degrades a server that fails
    /// to connect into a boot notice, never a fatal error.
    pub async fn connect(command: &str, args: &[String]) -> AgentResult<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd.env_clear();
        for key in INHERITED_ENV_VARS {
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }
        let transport = TokioChildProcess::new(cmd)
            .map_err(|error| AgentError::extensions(format!("spawn '{command}': {error}")))?;
        let service = timeout(CONNECT_TIMEOUT, ().serve(transport))
            .await
            .map_err(|_| {
                AgentError::extensions(format!(
                    "MCP handshake for '{command}' timed out after {CONNECT_TIMEOUT:?}"
                ))
            })?
            .map_err(|error| AgentError::extensions(format!("MCP handshake failed: {error}")))?;
        Ok(Self { service })
    }
}

#[async_trait::async_trait]
impl McpConnection for RmcpConnection {
    async fn list_tools(&self) -> AgentResult<Vec<McpToolSpec>> {
        let result = timeout(
            REQUEST_TIMEOUT,
            self.service.peer().list_tools(Default::default()),
        )
        .await
        .map_err(|_| {
            AgentError::extensions(format!("list_tools timed out after {REQUEST_TIMEOUT:?}"))
        })?
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
        let result = timeout(REQUEST_TIMEOUT, self.service.peer().call_tool(params))
            .await
            .map_err(|_| {
                AgentError::extensions(format!(
                    "call_tool '{name}' timed out after {REQUEST_TIMEOUT:?}"
                ))
            })?
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
