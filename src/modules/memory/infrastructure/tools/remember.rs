use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::memory::application::memory_port::MemoryPort;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{parse, parse_args};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::kernel::tool_call::ToolCall;

#[derive(Deserialize)]
struct RememberArgs {
    kind: String,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
    scope: String,
}

/// Tool that persists a memory entry to the project (`.kiri/memory/`) or shared
/// (`~/.kiri/memory/shared.db`) store, so durable knowledge survives across turns and sessions.
pub struct Remember {
    memory: Arc<dyn MemoryPort>,
    project_id: String,
}

impl Remember {
    pub fn new(memory: Arc<dyn MemoryPort>, project_id: String) -> Self {
        Self { memory, project_id }
    }
}

#[async_trait::async_trait(?Send)]
impl Tool for Remember {
    fn name(&self) -> &'static str {
        "remember"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Persist a durable memory entry. Use it to record knowledge worth keeping across turns and \
             sessions. 'kind' is one of: decision, pattern, anti-pattern, snippet, heuristic, fact. \
             'scope' is 'project' (this repo) or 'shared' (cross-project, high availability). Keep \
             'content' concise and self-contained (markdown allowed).",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["kind", "content", "scope"],
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["decision", "pattern", "anti-pattern", "snippet", "heuristic", "fact"],
                        "description": "The category of the entry."
                    },
                    "content": { "type": "string", "description": "The knowledge to store (markdown ok)." },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for retrieval."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "shared"],
                        "description": "Where to store: 'project' (this repo) or 'shared' (cross-project)."
                    }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        let a: RememberArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("remember {} ({})", a.kind, a.scope))
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        Some(confirm(
            format!("Gravar na memória. Aprova executar: {cmd}?"),
            true,
        ))
    }

    async fn execute(&self, _sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: RememberArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let Some(kind) = MemoryKind::from_str(&args.kind) else {
            return ToolOutcome::Error(format!(
                "invalid kind '{}': expected one of decision, pattern, anti-pattern, snippet, \
                 heuristic, fact",
                args.kind
            ));
        };
        let entry = MemoryEntry::new(
            kind,
            args.content,
            args.tags.into_iter().collect(),
            Some(self.project_id.clone()),
        );

        let result = match args.scope.as_str() {
            "project" => {
                if !self.memory.project_memory_available() {
                    return ToolOutcome::Error("project memory is unavailable".to_string());
                }
                self.memory.remember_project(entry).await
            }
            "shared" => {
                if !self.memory.shared_memory_available() {
                    return ToolOutcome::Error("shared memory is unavailable".to_string());
                }
                self.memory.remember_shared(entry).await
            }
            other => {
                return ToolOutcome::Error(format!(
                    "invalid scope '{other}': expected 'project' or 'shared'"
                ));
            }
        };

        match result {
            Ok(()) => ToolOutcome::Ok(format!("remembered {} in {} memory", args.kind, args.scope)),
            Err(error) => ToolOutcome::Error(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::infrastructure::test_support::{call, temp_port};
    use crate::modules::tools::infrastructure::sandbox::Sandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use tempfile::TempDir;

    fn sandbox() -> Sandbox {
        Sandbox::new(std::path::PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
    }

    #[tokio::test]
    async fn persists_then_recallable() {
        let dir = TempDir::new().unwrap();
        let port = temp_port(&dir).await;
        let tool = Remember::new(port.clone(), "proj-test".into());
        let sb = sandbox();

        let out = tool
            .execute(
                &sb,
                &call(
                    r#"{"kind":"fact","content":"edition 2024 ships in 1.85","scope":"project"}"#,
                ),
            )
            .await;
        assert!(matches!(out, ToolOutcome::Ok(_)));

        let hits = port.recall_project("edition", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn rejects_invalid_kind_and_scope() {
        let dir = TempDir::new().unwrap();
        let tool = Remember::new(temp_port(&dir).await, "proj-test".into());
        let sb = sandbox();

        let bad_kind = tool
            .execute(
                &sb,
                &call(r#"{"kind":"nope","content":"x","scope":"project"}"#),
            )
            .await;
        assert!(matches!(bad_kind, ToolOutcome::Error(_)));

        let bad_scope = tool
            .execute(
                &sb,
                &call(r#"{"kind":"fact","content":"x","scope":"galaxy"}"#),
            )
            .await;
        assert!(matches!(bad_scope, ToolOutcome::Error(_)));
    }
}
