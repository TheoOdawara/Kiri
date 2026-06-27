use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::memory::application::memory_port::MemoryPort;
use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{parse, parse_args};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::kernel::tool_call::ToolCall;

#[derive(Deserialize)]
struct RecallArgs {
    query: String,
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_scope() -> String {
    "both".to_string()
}

fn default_limit() -> usize {
    5
}

/// Read-only tool that recalls relevant memory entries (project, shared, or both) for a query, so the
/// model can pull prior decisions/patterns/facts into the current turn on demand.
pub struct RecallMemory {
    memory: Arc<dyn MemoryPort>,
}

impl RecallMemory {
    pub fn new(memory: Arc<dyn MemoryPort>) -> Self {
        Self { memory }
    }
}

#[async_trait::async_trait(?Send)]
impl Tool for RecallMemory {
    fn name(&self) -> &'static str {
        "recall_memory"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Recall relevant memory entries for a query. Memory holds durable knowledge across turns \
             and sessions: decisions, patterns, anti-patterns, snippets, heuristics, and facts. Scope \
             is 'project' (this repo's memory), 'shared' (cross-project memory), or 'both' (default). \
             Read-only.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "description": "What to search for in memory." },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "shared", "both"],
                        "description": "Which memory to search. Defaults to 'both'."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Max entries per scope. Defaults to 5."
                    }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, call: &ToolCall) -> Option<String> {
        let a: RecallArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("recall_memory {}", a.query))
    }

    fn confirmation(&self, sandbox: &Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        Some(confirm(
            format!("Consultar a memória. Aprova executar: {cmd}?"),
            true,
        ))
    }

    async fn execute(&self, _sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: RecallArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        // A blank query matches everything (substring of "" is always true), letting the model dump the
        // whole store; reject it, mirroring the guard DocsLibrary already has.
        if args.query.trim().is_empty() {
            return ToolOutcome::Error("query must not be empty".to_string());
        }
        let scope = args.scope.as_str();
        if !matches!(scope, "project" | "shared" | "both") {
            return ToolOutcome::Error(format!(
                "invalid scope '{scope}': expected 'project', 'shared', or 'both'"
            ));
        }

        let mut sections: Vec<String> = Vec::new();
        if matches!(scope, "project" | "both") && self.memory.project_memory_available() {
            match self.memory.recall_project(&args.query, args.limit).await {
                Ok(entries) if !entries.is_empty() => {
                    sections.push(render("Project memory", &entries))
                }
                Ok(_) => {}
                Err(error) => return ToolOutcome::Error(error.to_string()),
            }
        }
        if matches!(scope, "shared" | "both") && self.memory.shared_memory_available() {
            match self.memory.recall_shared(&args.query, args.limit).await {
                Ok(entries) if !entries.is_empty() => {
                    sections.push(render("Shared memory", &entries))
                }
                Ok(_) => {}
                Err(error) => return ToolOutcome::Error(error.to_string()),
            }
        }

        if sections.is_empty() {
            ToolOutcome::Ok("No matching memory entries.".to_string())
        } else {
            ToolOutcome::Ok(sections.join("\n\n"))
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

fn render(heading: &str, entries: &[MemoryEntry]) -> String {
    let rendered = entries
        .iter()
        .map(MemoryEntry::format_for_context)
        .collect::<Vec<_>>()
        .join("\n");
    format!("# {heading}\n{rendered}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::MemoryKind;
    use crate::modules::memory::infrastructure::test_support::{call, temp_port};
    use crate::modules::tools::infrastructure::sandbox::Sandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use tempfile::TempDir;

    fn sandbox() -> Sandbox {
        Sandbox::new(std::path::PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
    }

    #[tokio::test]
    async fn recalls_after_remember() {
        let dir = TempDir::new().unwrap();
        let port = temp_port(&dir).await;
        port.remember_shared(MemoryEntry::new(
            MemoryKind::Heuristic,
            "fail fast on bad input".into(),
            Default::default(),
            Some("p".into()),
        ))
        .await
        .unwrap();

        let tool = RecallMemory::new(port);
        let out = tool
            .execute(&sandbox(), &call(r#"{"query":"fail","scope":"shared"}"#))
            .await;
        match out {
            ToolOutcome::Ok(body) => assert!(body.contains("fail fast on bad input")),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_result_and_invalid_scope() {
        let dir = TempDir::new().unwrap();
        let tool = RecallMemory::new(temp_port(&dir).await);

        let empty = tool
            .execute(&sandbox(), &call(r#"{"query":"nothing"}"#))
            .await;
        assert_eq!(
            empty,
            ToolOutcome::Ok("No matching memory entries.".to_string())
        );

        let bad = tool
            .execute(&sandbox(), &call(r#"{"query":"x","scope":"nope"}"#))
            .await;
        assert!(matches!(bad, ToolOutcome::Error(_)));
    }
}
