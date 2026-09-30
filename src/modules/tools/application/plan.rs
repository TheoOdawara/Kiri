use serde::Deserialize;

use crate::shared::kernel::tool_call::ToolCall;

/// The stable name the model calls to submit a finished plan for approval. Shared by the agent loop
/// (which intercepts the call instead of executing it) and the `PresentPlan` adapter that advertises it.
pub const PRESENT_PLAN: &str = "present_plan";

#[derive(Deserialize)]
struct PlanArgs {
    plan: String,
}

/// Extract the plan text from a `present_plan` call's arguments. `None` when the args do not parse —
/// the agent loop falls back to the turn's narration content so a finished plan is never lost.
pub fn extract_plan(call: &ToolCall) -> Option<String> {
    let args: PlanArgs = serde_json::from_str(call.function.arguments.as_str()).ok()?;
    Some(args.plan)
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
}
