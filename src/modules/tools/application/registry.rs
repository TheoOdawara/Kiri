use std::sync::Arc;

use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{Confirmation, Tool, ToolOutcome};
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::tool_call::ToolCall;

/// Holds the registered tools and dispatches by name. Replaces the central `tool_definitions`/
/// `execute`/`confirmation_prompt` match: a tool advertises, confirms, and runs itself. Tools are `Arc`
/// (not `Box`) so the same instances can be cheaply shared into a filtered child registry for a
/// dispatched subagent (ADR 0029) — a stateful tool (e.g. an MCP proxy) is never rebuilt or
/// double-connected just to hand a subset to a nested `AgentLoop`.
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self { tools }
    }

    /// The schema array advertised to the provider (replaces `tool_definitions()`). Plan-only tools
    /// (e.g. `present_plan`) are a planning control surface, not a filesystem action, so they are
    /// withheld outside plan mode; `schemas_for(Plan)` re-includes them via `is_plannable`.
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .iter()
            .filter(|tool| !tool.plan_only())
            .map(|tool| tool.schema())
            .collect()
    }

    /// The schema array advertised for `mode`. In plan mode only plannable tools are offered, so the
    /// model can investigate (read files, run dev servers, search logs) but not mutate the project
    /// directly — `run_command` is plannable but its plan-mode allow-list permits only safe
    /// inspection/build/test programs at execution time.
    pub fn schemas_for(&self, mode: ApprovalMode) -> Vec<serde_json::Value> {
        if mode == ApprovalMode::Plan {
            self.tools
                .iter()
                .filter(|tool| tool.is_plannable())
                .map(|tool| tool.schema())
                .collect()
        } else {
            self.schemas()
        }
    }

    /// Whether a named tool exists and mutates the filesystem. The engine path gates on
    /// `is_plannable` instead, so the only caller is the classification test; gated `#[cfg(test)]`
    /// so it never ships in the release binary.
    #[cfg(test)]
    pub fn is_destructive(&self, name: &str) -> bool {
        self.find(name).is_some_and(|tool| !tool.is_read_only())
    }

    /// Whether a named tool is advertised in plan mode. Plannable tools either never mutate
    /// (`is_read_only`) or opt in explicitly (`is_plannable` override, e.g. `run_command`).
    pub fn is_plannable(&self, name: &str) -> bool {
        self.find(name).is_some_and(|tool| tool.is_plannable())
    }

    /// Whether a named tool must be confirmed even in auto mode (irreversible / high blast radius).
    /// An unknown tool is not gated — `execute` reports the unknown-tool error instead.
    pub fn confirm_in_auto(&self, name: &str) -> bool {
        self.find(name).is_some_and(|tool| tool.confirm_in_auto())
    }

    /// In plan mode, ask the named tool whether the call should be blocked. Returns
    /// `Some(reason)` if the tool refuses the call, `None` if it's allowed.
    pub fn plan_check(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        self.find(&call.function.name)?.plan_check(sandbox, call)
    }

    fn find(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .map(|tool| tool.as_ref())
            .find(|tool| tool.name() == name)
    }

    pub fn confirm(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        self.find(&call.function.name)?.confirmation(sandbox, call)
    }

    /// The bare command label for a call, for on-screen display. `None` for an unknown tool or
    /// unparseable args (the caller falls back to the tool name).
    pub fn command_line(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        self.find(&call.function.name)?.command_line(sandbox, call)
    }

    pub async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        match self.find(&call.function.name) {
            Some(tool) => tool.execute(sandbox, call).await,
            None => ToolOutcome::Error(format!("unknown tool '{}'", call.function.name)),
        }
    }
}

#[path = "registry_tests.rs"]
#[cfg(test)]
mod tests;
