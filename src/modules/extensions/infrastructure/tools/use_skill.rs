use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::extensions::domain::resource::Skill;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, function_schema,
};
use crate::modules::tools::infrastructure::args::{parse, parse_args};
use crate::shared::kernel::tool_call::ToolCall;

#[derive(Deserialize)]
struct UseSkillArgs {
    name: String,
}

/// Read-only tool that fetches a loaded skill's full body by id (ADR 0021). The system prompt carries
/// only each skill's one-line description (`ExtensionCatalog::skills_index`); the body is fetched here
/// on demand, so the base prompt stays lean regardless of how many skills are installed.
pub struct UseSkill {
    skills: Arc<HashMap<String, Skill>>,
}

impl UseSkill {
    pub fn new(skills: Arc<HashMap<String, Skill>>) -> Self {
        Self { skills }
    }
}

#[async_trait::async_trait(?Send)]
impl Tool for UseSkill {
    fn name(&self) -> &'static str {
        "use_skill"
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Fetch the full instructions body of a loaded skill by its id, listed in the '# Skills' \
             section of this prompt. Read-only.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["name"],
                "properties": {
                    "name": { "type": "string", "description": "The skill's id, as listed in '# Skills'." }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let a: UseSkillArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("use_skill {}", a.name))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        Some(confirm(
            format!("Consultar a skill. Aprova executar: {cmd}?"),
            true,
        ))
    }

    async fn execute(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: UseSkillArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        match self.skills.get(&args.name) {
            Some(skill) => ToolOutcome::Ok(skill.body.clone()),
            None => {
                let mut known: Vec<&str> = self.skills.keys().map(String::as_str).collect();
                known.sort_unstable();
                ToolOutcome::Error(format!(
                    "unknown skill '{}'; loaded skills: {}",
                    args.name,
                    known.join(", ")
                ))
            }
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::extensions::domain::scope::Layer;
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::tool_call::FunctionCall;

    /// `use_skill` never touches the sandbox path — it only reads its own in-memory skills map — so a
    /// bare root is enough to satisfy the `Tool` API's `execute`/`confirmation` signatures.
    fn sandbox() -> FsSandbox {
        FsSandbox::new(std::path::PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
    }

    fn call(arguments: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "use_skill".to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn skill(id: &str, body: &str) -> Skill {
        Skill {
            id: id.to_string(),
            description: format!("{id} description"),
            body: body.to_string(),
            layer: Layer::Global,
            path: format!("/fake/{id}.md"),
            tags: Default::default(),
            script: None,
        }
    }

    #[tokio::test]
    async fn returns_the_body_of_a_known_skill() {
        let mut skills = HashMap::new();
        skills.insert(
            "pdf-extract".to_string(),
            skill("pdf-extract", "Use pdftotext."),
        );
        let tool = UseSkill::new(Arc::new(skills));
        let out = tool
            .execute(&sandbox(), &call(r#"{"name":"pdf-extract"}"#))
            .await;
        assert_eq!(out, ToolOutcome::Ok("Use pdftotext.".to_string()));
    }

    #[tokio::test]
    async fn unknown_skill_lists_what_is_loaded() {
        let mut skills = HashMap::new();
        skills.insert("a".to_string(), skill("a", "A body"));
        let tool = UseSkill::new(Arc::new(skills));
        let out = tool.execute(&sandbox(), &call(r#"{"name":"ghost"}"#)).await;
        match out {
            ToolOutcome::Error(message) => {
                assert!(message.contains("unknown skill 'ghost'"));
                assert!(message.contains("a"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn is_read_only() {
        let tool = UseSkill::new(Arc::new(HashMap::new()));
        assert!(tool.is_read_only());
    }
}
