use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::tools::application::tool::{Confirmation, Tool, ToolOutcome, function_schema};
use crate::modules::tools::infrastructure::args::parse;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::kernel::tool_call::ToolCall;

/// The stable name the model calls to submit a finished plan for approval. Shared with the agent loop,
/// which intercepts the call instead of executing it against the sandbox.
pub const PRESENT_PLAN: &str = "present_plan";

#[derive(Deserialize)]
struct PlanArgs {
    plan: String,
}

/// Extract the plan text from a `present_plan` call's arguments. `None` when the args do not parse —
/// the agent loop falls back to the turn's narration content so a finished plan is never lost.
pub fn extract_plan(call: &ToolCall) -> Option<String> {
    let args: PlanArgs = parse(call.function.arguments.as_str()).ok()?;
    Some(args.plan)
}

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
    use crate::shared::kernel::tool_call::FunctionCall;

    fn call(args: &str) -> ToolCall {
        ToolCall {
            id: "c1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: PRESENT_PLAN.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[test]
    fn extract_plan_reads_the_plan_argument() {
        let c = call("{\"plan\":\"Plano\\n1. fazer X\"}");
        assert_eq!(extract_plan(&c).as_deref(), Some("Plano\n1. fazer X"));
    }

    #[test]
    fn extract_plan_is_none_for_unparseable_args() {
        assert!(extract_plan(&call("{not json")).is_none());
        assert!(extract_plan(&call(r#"{"wrong":1}"#)).is_none());
    }

    #[test]
    fn present_plan_is_plan_only_and_plannable() {
        let tool = PresentPlan;
        assert!(tool.plan_only());
        assert!(tool.is_plannable());
        assert!(!tool.is_read_only());
    }
}
