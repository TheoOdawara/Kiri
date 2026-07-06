use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::memory::application::memory_port::Memory;
use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::memory::domain::scope::RecallScope;
use crate::modules::memory::domain::similarity::is_near_duplicate;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{parse, parse_args};
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

/// Hard ceiling on the model-supplied `limit`. The schema's `"minimum": 1` is advisory only (not
/// enforced by the JSON parser), and the cross-store dedup below is O(project × shared) — an unbounded
/// limit against a since-grown store would turn one tool call into real CPU cost (ADR 0023).
const MAX_LIMIT: usize = 50;

/// Read-only tool that recalls relevant memory entries (project, shared, or both) for a query, so the
/// model can pull prior decisions/patterns/facts into the current turn on demand.
pub struct RecallMemory {
    memory: Arc<dyn Memory>,
}

impl RecallMemory {
    pub fn new(memory: Arc<dyn Memory>) -> Self {
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

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let a: RecallArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("recall_memory {}", a.query))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        Some(confirm(
            format!("Consultar a memória. Aprova executar: {cmd}?"),
            true,
        ))
    }

    async fn execute(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: RecallArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        // A blank query matches everything (substring of "" is always true), letting the model dump the
        // whole store; reject it, mirroring the guard DocsLibrary already has.
        if args.query.trim().is_empty() {
            return ToolOutcome::Error("query must not be empty".to_string());
        }
        let Some(scope) = RecallScope::from_wire(&args.scope) else {
            return ToolOutcome::Error(format!(
                "invalid scope '{}': expected 'project', 'shared', or 'both'",
                args.scope
            ));
        };

        let limit = args.limit.min(MAX_LIMIT);
        let mut project_entries = Vec::new();
        if scope.includes_project() && self.memory.project_memory_available() {
            match self.memory.recall_project(&args.query, limit).await {
                Ok(entries) => project_entries = entries,
                Err(error) => return ToolOutcome::Error(error.to_string()),
            }
        }
        let mut shared_entries = Vec::new();
        if scope.includes_shared() && self.memory.shared_memory_available() {
            match self.memory.recall_shared(&args.query, limit).await {
                Ok(entries) => shared_entries = entries,
                Err(error) => return ToolOutcome::Error(error.to_string()),
            }
        }

        // Cross-store provenance: project wins (ADR 0023). A fact present in both stores — e.g. the
        // distiller wrote it to both, or the user entered it twice — surfaces once, from the project
        // entry, rather than duplicated across both sections. The drop is counted, never silent: the
        // model is told when a shared entry was withheld, mirroring the distiller's `DistillReport.skipped`.
        let shared_before = shared_entries.len();
        shared_entries.retain(|shared| {
            !project_entries
                .iter()
                .any(|project| is_near_duplicate(&project.content, &shared.content))
        });
        let dropped = shared_before - shared_entries.len();

        let mut sections: Vec<String> = Vec::new();
        if !project_entries.is_empty() {
            sections.push(render("Project memory", &project_entries));
        }
        if !shared_entries.is_empty() {
            sections.push(render("Shared memory", &shared_entries));
        }
        if dropped > 0 {
            sections.push(format!(
                "({dropped} shared entr{} omitted as duplicate of project memory)",
                if dropped == 1 { "y" } else { "ies" }
            ));
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
    use crate::modules::memory::infrastructure::test_support::{call, sandbox, temp_port};
    use tempfile::TempDir;

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

    #[tokio::test]
    async fn cross_store_near_duplicate_surfaces_once_favoring_project() {
        let dir = TempDir::new().unwrap();
        let port = temp_port(&dir).await;
        port.remember_project(MemoryEntry::new(
            MemoryKind::Fact,
            "the api times out after 30 seconds".into(),
            Default::default(),
            Some("p".into()),
        ))
        .await
        .unwrap();
        // Same fact, different casing/spacing — normalizes equal to the project entry above. Written to
        // the shared store, e.g. by the distiller classifying the same fact into both scopes across two
        // sessions.
        port.remember_shared(MemoryEntry::new(
            MemoryKind::Fact,
            "The API   times out after 30 seconds".into(),
            Default::default(),
            Some("p".into()),
        ))
        .await
        .unwrap();
        // An unrelated shared entry must still surface — the dedup only drops what near-duplicates a
        // project entry, not the whole shared section.
        port.remember_shared(MemoryEntry::new(
            MemoryKind::Fact,
            "the api rate limit is 100 requests per minute".into(),
            Default::default(),
            Some("p".into()),
        ))
        .await
        .unwrap();

        let tool = RecallMemory::new(port);
        let out = tool
            .execute(&sandbox(), &call(r#"{"query":"api","scope":"both"}"#))
            .await;
        match out {
            ToolOutcome::Ok(body) => {
                // Exactly two entries render (project's + the unrelated shared one) — not three. A
                // literal-substring check on lowercase content would pass whether or not the retain
                // actually fired (the shared duplicate here differs in case/spacing), so this counts the
                // rendered `MemoryEntry` markers directly.
                assert_eq!(
                    body.matches("--- MemoryEntry").count(),
                    2,
                    "the cross-store duplicate must be dropped: {body}"
                );
                assert!(
                    !body.contains("The API   times out after 30 seconds"),
                    "the shared duplicate's own rendering must not appear: {body}"
                );
                assert!(body.contains("Project memory"));
                assert!(
                    body.contains("rate limit"),
                    "an unrelated shared entry must still surface: {body}"
                );
                assert!(
                    body.contains("1 shared entry omitted"),
                    "the drop must be observable, not silent: {body}"
                );
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }
}
