//! A discovered MCP tool registers into the `ToolRegistry` exactly like a built-in file tool, so it
//! passes through the same approval gate.

use std::sync::Arc;

use serde_json::Value;

use crate::modules::mcp::application::mcp_connection::{McpConnection, McpToolSpec};
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, confirm_execute_suffix, function_schema,
};
use crate::shared::kernel::tool_call::ToolCall;

/// `qualified_name` is leaked once at boot to satisfy `Tool::name`'s `&'static str`. Tools are discovered
/// once per session, so the leak is bounded.
pub struct McpToolProxy {
    qualified_name: &'static str,
    /// The name the server knows, which `call_tool` must send back — `qualified_name` is namespaced.
    remote_name: String,
    description: String,
    input_schema: Value,
    connection: Arc<dyn McpConnection>,
}

impl McpToolProxy {
    /// Namespaces the advertised name so two servers can never collide on a shared tool name.
    pub fn new(server_id: &str, spec: McpToolSpec, connection: Arc<dyn McpConnection>) -> Self {
        let qualified = format!("mcp__{server_id}__{}", spec.name);
        let qualified_name: &'static str = Box::leak(qualified.into_boxed_str());
        Self {
            qualified_name,
            remote_name: spec.name,
            description: spec.description,
            input_schema: spec.input_schema,
            connection,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Tool for McpToolProxy {
    fn name(&self) -> &'static str {
        self.qualified_name
    }

    fn schema(&self) -> Value {
        function_schema(
            self.qualified_name,
            &self.description,
            self.input_schema.clone(),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, _call: &ToolCall) -> Option<String> {
        Some(self.qualified_name.to_string())
    }

    fn confirmation(&self, _sandbox: &dyn Sandbox, _call: &ToolCall) -> Option<Confirmation> {
        Some(confirm(
            format!(
                "Chamar a ferramenta MCP. {}",
                confirm_execute_suffix(self.qualified_name)
            ),
            true,
        ))
    }

    async fn execute(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: Value = match serde_json::from_str(call.function.arguments.as_str()) {
            Ok(value) => value,
            Err(error) => return ToolOutcome::Error(format!("invalid arguments: {error}")),
        };
        match self.connection.call_tool(&self.remote_name, args).await {
            Ok(text) => ToolOutcome::Ok(text),
            Err(error) => ToolOutcome::Error(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::error::AgentResult;
    use crate::shared::kernel::tool_call::FunctionCall;
    use std::sync::Mutex;

    /// A connection double recording the last call, returning a fixed outcome.
    struct FakeConnection {
        last_call: Mutex<Option<(String, Value)>>,
        result: Result<String, String>,
    }

    #[async_trait::async_trait]
    impl McpConnection for FakeConnection {
        async fn list_tools(&self) -> AgentResult<Vec<McpToolSpec>> {
            Ok(Vec::new())
        }

        async fn call_tool(&self, name: &str, arguments: Value) -> AgentResult<String> {
            *self.last_call.lock().unwrap() = Some((name.to_string(), arguments));
            match &self.result {
                Ok(text) => Ok(text.clone()),
                Err(error) => Ok(error.clone()), // mirrors the real adapter: tool errors are `Ok(text)`
            }
        }
    }

    fn sandbox() -> FsSandbox {
        FsSandbox::new(std::path::PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
    }

    fn call(arguments: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "mcp__fs__read_file".to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn name_is_namespaced_by_server_id() {
        let conn = Arc::new(FakeConnection {
            last_call: Mutex::new(None),
            result: Ok(String::new()),
        });
        let proxy = McpToolProxy::new(
            "fs",
            McpToolSpec {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            conn,
        );
        assert_eq!(proxy.name(), "mcp__fs__read_file");
    }

    #[tokio::test]
    async fn execute_forwards_the_remote_name_and_parsed_args() {
        let conn = Arc::new(FakeConnection {
            last_call: Mutex::new(None),
            result: Ok("file contents".to_string()),
        });
        let proxy = McpToolProxy::new(
            "fs",
            McpToolSpec {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            conn.clone(),
        );
        let outcome = proxy
            .execute(&sandbox(), &call(r#"{"path":"a.txt"}"#))
            .await;
        assert_eq!(outcome, ToolOutcome::Ok("file contents".to_string()));
        let recorded = conn.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.0, "read_file"); // the bare remote name, not the qualified one
        assert_eq!(recorded.1, serde_json::json!({"path": "a.txt"}));
    }

    #[tokio::test]
    async fn invalid_arguments_are_reported_without_calling_the_connection() {
        let conn = Arc::new(FakeConnection {
            last_call: Mutex::new(None),
            result: Ok(String::new()),
        });
        let proxy = McpToolProxy::new(
            "fs",
            McpToolSpec {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            conn.clone(),
        );
        let outcome = proxy.execute(&sandbox(), &call("not json")).await;
        assert!(matches!(outcome, ToolOutcome::Error(_)));
        assert!(conn.last_call.lock().unwrap().is_none());
    }
}
