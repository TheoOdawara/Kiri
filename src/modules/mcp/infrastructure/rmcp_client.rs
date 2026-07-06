//! The sanctioned site for the `mcp` context's process/network I/O (ADR 0021): spawns an MCP server as a
//! child process (stdio transport) via the official `rmcp` SDK and speaks the protocol over it. One
//! adapter, `RmcpConnection`, kept alive for the session's lifetime once connected.

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

/// Bound on the handshake (spawn + MCP `initialize`); a hung/malicious server's process must not stall
/// `app::wire` forever — every other boot degradation already turns into a notice, this must too.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Bound on any single request over an established connection (`list_tools`/`call_tool`); a hung remote
/// call must not strand a turn's busy state (the project's "all I/O has a timeout" non-negotiable).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Non-secret env vars a spawned server needs to resolve/run typical CLI tools (node/npx/python/...).
/// Re-added after `env_clear()` so nothing else — provider API keys, credentials, sandbox flags — leaks
/// into an approved project-layer server's process; ADR 0021 promises "never receives a harness secret",
/// and `tokio::process::Command` inherits the *entire* parent env by default, so this must be explicit.
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

/// A live connection to one MCP server, spawned as a child process over stdio. `Send`-bound (rmcp's
/// service handle is `Send + Sync`), unlike the engine's `?Send` ports — this context has no `&dyn
/// Sandbox`/`!Send` value crossing an await point.
pub struct RmcpConnection {
    service: RunningService<RoleClient, ()>,
}

impl RmcpConnection {
    /// Spawn `command args...` and complete the MCP handshake over its stdio. A spawn failure, a
    /// handshake failure, or a handshake that exceeds `CONNECT_TIMEOUT` all surface as
    /// `AgentError::Extensions` — the caller (`app::wire`) treats a server that fails to connect like any
    /// other auxiliary degradation: a boot notice, never fatal, never a hang.
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
