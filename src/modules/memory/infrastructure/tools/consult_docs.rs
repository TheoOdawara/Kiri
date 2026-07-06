use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::memory::infrastructure::docs_library::DocsLibrary;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, confirm_execute_suffix, function_schema,
};
use crate::modules::tools::infrastructure::args::{parse, parse_args};
use crate::shared::kernel::tool_call::ToolCall;

#[derive(Deserialize)]
struct ConsultArgs {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    5
}

/// Read-only tool that consults the project's documentation tree (default `<workspace>/docs`) as a
/// fallback knowledge source when memory does not cover a question. Returns ranked excerpts.
pub struct ConsultDocs {
    docs: Arc<DocsLibrary>,
}

impl ConsultDocs {
    pub fn new(docs: Arc<DocsLibrary>) -> Self {
        Self { docs }
    }
}

#[async_trait::async_trait(?Send)]
impl Tool for ConsultDocs {
    fn name(&self) -> &'static str {
        "consult_docs"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Consult the project's documentation (ADRs, requirements, guides under docs/) for a query, \
             returning the most relevant excerpts with their file paths. Use it as a fallback when \
             memory does not cover what you need; read the cited file in full for more. Read-only.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "description": "What to look for in the docs." },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Max excerpts to return. Defaults to 5."
                    }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let a: ConsultArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("consult_docs {}", a.query))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        Some(confirm(
            format!("Consultar a documentação. {}", confirm_execute_suffix(&cmd)),
            true,
        ))
    }

    async fn execute(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: ConsultArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        if !self.docs.is_available() {
            return ToolOutcome::Ok("No documentation directory found.".to_string());
        }
        match self.docs.search(&args.query, args.limit).await {
            Ok(matches) if matches.is_empty() => {
                ToolOutcome::Ok("No matching documentation.".to_string())
            }
            Ok(matches) => {
                let body = matches
                    .iter()
                    .map(|m| format!("## {}\n{}", m.path, m.excerpt))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                ToolOutcome::Ok(body)
            }
            Err(error) => ToolOutcome::Error(error.to_string()),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::infrastructure::test_support::{call, sandbox};
    use tempfile::TempDir;

    #[tokio::test]
    async fn finds_matching_docs() {
        let dir = TempDir::new().unwrap();
        let docs_root = dir.path().join("docs");
        tokio::fs::create_dir_all(&docs_root).await.unwrap();
        tokio::fs::write(docs_root.join("arch.md"), "Modular hexagonal architecture.")
            .await
            .unwrap();

        let tool = ConsultDocs::new(Arc::new(DocsLibrary::new(docs_root)));
        let out = tool
            .execute(&sandbox(), &call(r#"{"query":"hexagonal"}"#))
            .await;
        match out {
            ToolOutcome::Ok(body) => assert!(body.to_lowercase().contains("hexagonal")),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_docs_dir_is_graceful() {
        let dir = TempDir::new().unwrap();
        let tool = ConsultDocs::new(Arc::new(DocsLibrary::new(dir.path().join("nope"))));
        let out = tool.execute(&sandbox(), &call(r#"{"query":"x"}"#)).await;
        assert_eq!(
            out,
            ToolOutcome::Ok("No documentation directory found.".to_string())
        );
    }
}
