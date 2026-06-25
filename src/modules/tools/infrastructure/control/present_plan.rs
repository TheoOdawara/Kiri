use serde_json::{Value, json};

use crate::modules::tools::application::plan::{PRESENT_PLAN, extract_plan};
use crate::modules::tools::application::tool::{Confirmation, Tool, ToolOutcome, function_schema};
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::kernel::tool_call::ToolCall;

/// The plan-mode control tool: the model calls it once, with the complete plan as a single markdown
/// string, to surface the plan for the user's approval. It performs no filesystem action — the agent
/// loop intercepts the call, ends the planning turn, and the TUI renders the plan and an approval box.
/// Advertised only in plan mode (`plan_only`).
pub struct PresentPlan;

#[async_trait::async_trait(?Send)]
impl Tool for PresentPlan {
    fn name(&self) -> &'static str {
        PRESENT_PLAN
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Submit your finished plan for the user's approval. Call this exactly once, only after you \
             have investigated and the plan is complete. Pass the entire plan as a single markdown \
             string in `plan`. This is the only way to present a plan; do not also write the plan as \
             ordinary prose. Available only in plan mode.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["plan"],
                "properties": {
                    "plan": {
                        "type": "string",
                        "description": "The complete plan as a markdown document."
                    }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &Sandbox, _call: &ToolCall) -> Option<String> {
        Some("apresentar plano".to_string())
    }

    fn confirmation(&self, _sandbox: &Sandbox, _call: &ToolCall) -> Option<Confirmation> {
        // Never confirmed as an ordinary tool: the agent loop intercepts the call and routes it to the
        // plan-approval box instead of the per-call confirmation flow.
        None
    }

    async fn execute(&self, _sandbox: &Sandbox, call: &ToolCall) -> ToolOutcome {
        // Unreachable on the normal path — the agent loop handles `present_plan` before dispatching to
        // the registry. Kept total (echoes the plan) so a stray call can never panic or mutate state.
        ToolOutcome::Ok(extract_plan(call).unwrap_or_default())
    }

    fn is_plannable(&self) -> bool {
        true
    }

    fn plan_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_plan_is_plan_only_and_plannable() {
        let tool = PresentPlan;
        assert!(tool.plan_only());
        assert!(tool.is_plannable());
        assert!(!tool.is_read_only());
    }
}
