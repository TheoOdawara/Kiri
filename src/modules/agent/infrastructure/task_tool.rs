//! The `task` tool (ADR 0029): hands a read-only sub-task to a nested `AgentLoop` in an isolated context.
//! A subagent's toolset intersects its profile's `allowed-tools` with the parent's read-only tools, so a
//! dispatched turn can never write, delete, or run a shell. Depth is capped at 1 structurally: the pool it
//! draws from never contains `task` itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::agent::application::agent_loop::{AgentLoop, TurnOutcome};
use crate::modules::agent::application::approval_policy::{
    Approval, ApprovalPolicy, CheckpointReason,
};
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::application::tool_observer::ToolObserver;
use crate::modules::extensions::domain::resource::AgentProfile;
use crate::modules::provider::application::completion_provider::{CompletionProvider, EventSink};
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::{
    Confirmation, Tool, ToolOutcome, confirm, confirm_execute_suffix, function_schema,
};
use crate::modules::tools::infrastructure::args::{parse, parse_args};
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::conversation::Conversation;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::role::Role;
use crate::shared::kernel::stream_event::StreamEvent;
use crate::shared::kernel::tool_call::ToolCall;

#[derive(Deserialize)]
struct TaskArgs {
    agent: String,
    prompt: String,
}

/// Engine IO for a dispatched subagent, where nobody is at the keyboard. `EventSink`/`Presenter`/
/// `ToolObserver` are no-ops, so a nested stream can never interleave with the parent's. `ApprovalPolicy`
/// is the security-load-bearing part: it approves only what auto-mode would default-accept (an in-root
/// read) and declines everything else outright (SEC-01 — `~/.ssh/id_rsa` is refused, not silently
/// confirmed), ending the turn at the first checkpoint rather than running unbounded with no one to ask.
struct HeadlessIo;

impl EventSink for HeadlessIo {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

impl Presenter for HeadlessIo {
    fn begin_round(&mut self) {}
    fn finish_round(&mut self) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for HeadlessIo {
    async fn decide(&mut self, confirmation: &Confirmation) -> Approval {
        if confirmation.default_accept {
            Approval::Approved
        } else {
            Approval::Declined
        }
    }

    async fn confirm_continue(&mut self, _reason: CheckpointReason) -> Approval {
        Approval::Declined
    }
}

impl ToolObserver for HeadlessIo {
    fn tool_started(&mut self, _call: &ToolCall, _command: &str) {}
    fn tool_finished(&mut self, _call: &ToolCall, _outcome: &ToolOutcome, _elapsed: Duration) {}
}

/// Dispatches a loaded `AgentProfile` as a nested, isolated `AgentLoop` turn. The tool pool injected here
/// is every tool in the parent registry except `task` itself.
pub struct TaskTool {
    provider: Arc<dyn CompletionProvider>,
    child_tools: Vec<Arc<dyn Tool>>,
    agents: Arc<HashMap<String, AgentProfile>>,
    default_model: String,
    checkpoint_budget: Duration,
    max_tool_calls: usize,
}

impl TaskTool {
    pub fn new(
        provider: Arc<dyn CompletionProvider>,
        child_tools: Vec<Arc<dyn Tool>>,
        agents: Arc<HashMap<String, AgentProfile>>,
        default_model: String,
        checkpoint_budget: Duration,
        max_tool_calls: usize,
    ) -> Self {
        Self {
            provider,
            child_tools,
            agents,
            default_model,
            checkpoint_budget,
            max_tool_calls,
        }
    }

    /// An empty `allowed-tools` means every read-only tool. The `is_read_only` intersection is v1's
    /// security boundary: a profile naming `write_file` never gets it, because a headless subagent has no
    /// live user to confirm an irreversible action.
    fn tools_for(&self, profile: &AgentProfile) -> Vec<Arc<dyn Tool>> {
        self.child_tools
            .iter()
            .filter(|tool| tool.name() != Self::NAME && tool.is_read_only())
            .filter(|tool| {
                profile.allowed_tools.is_empty()
                    || profile.allowed_tools.iter().any(|name| name == tool.name())
            })
            .cloned()
            .collect()
    }

    const NAME: &'static str = "task";
}

/// The subagent's answer. `None` if the last message is not a non-empty assistant text — unreachable
/// after `TurnOutcome::Completed`, but defensive rather than indexing blindly.
fn last_assistant_text(conversation: &Conversation) -> Option<String> {
    let last = conversation.messages().last()?;
    if last.role != Role::Assistant {
        return None;
    }
    last.content.clone().filter(|text| !text.is_empty())
}

#[async_trait::async_trait(?Send)]
impl Tool for TaskTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn schema(&self) -> Value {
        function_schema(
            self.name(),
            "Dispatch a loaded read-only agent profile (see '# Agents') as an isolated subagent turn. \
             Use it to hand off a self-contained search or planning sub-task instead of doing it inline. \
             The subagent cannot write, delete, or run a shell; it returns its final answer as text.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["agent", "prompt"],
                "properties": {
                    "agent": { "type": "string", "description": "The agent's id, as listed in '# Agents'." },
                    "prompt": { "type": "string", "description": "The task for the subagent to perform." }
                }
            }),
        )
    }

    fn command_line(&self, _sandbox: &dyn Sandbox, call: &ToolCall) -> Option<String> {
        let a: TaskArgs = parse(call.function.arguments.as_str()).ok()?;
        Some(format!("task {}", a.agent))
    }

    fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
        let cmd = self.command_line(sandbox, call)?;
        Some(confirm(
            format!("Despachar subagente. {}", confirm_execute_suffix(&cmd)),
            true,
        ))
    }

    async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
        let args: TaskArgs = match parse_args(call) {
            Ok(args) => args,
            Err(out) => return out,
        };
        let Some(profile) = self.agents.get(&args.agent) else {
            let mut known: Vec<&str> = self.agents.keys().map(String::as_str).collect();
            known.sort_unstable();
            return ToolOutcome::Error(format!(
                "unknown agent '{}'; loaded agents: {}",
                args.agent,
                known.join(", ")
            ));
        };

        let registry = ToolRegistry::new(self.tools_for(profile));
        let model = profile
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let sub = AgentLoop::new(
            self.provider.clone(),
            registry,
            model,
            self.checkpoint_budget,
            self.max_tool_calls,
        );

        let mut conversation = Conversation::new(profile.system_prompt.clone());
        conversation.push(Message::user(args.prompt));
        let mut io = HeadlessIo;

        match sub
            .run(&mut conversation, sandbox, ApprovalMode::Auto, &mut io)
            .await
        {
            Ok(TurnOutcome::Completed) => match last_assistant_text(&conversation) {
                Some(text) => ToolOutcome::Ok(text),
                None => ToolOutcome::Error(format!(
                    "subagent '{}' produced no text response",
                    args.agent
                )),
            },
            Ok(TurnOutcome::PlanProposed(plan)) => ToolOutcome::Ok(plan),
            Ok(TurnOutcome::Aborted) => {
                ToolOutcome::Error(format!("subagent '{}' aborted", args.agent))
            }
            Err(error) => ToolOutcome::Error(format!("subagent '{}' failed: {error}", args.agent)),
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
    use crate::modules::provider::application::completion_provider::TurnRequest;
    use crate::modules::tools::infrastructure::sandbox::FsSandbox;
    use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
    use crate::shared::kernel::completed_turn::CompletedTurn;
    use crate::shared::kernel::tool_call::FunctionCall;
    use std::sync::Mutex;

    /// Read-only-ness is a constructor parameter, so the security boundary can be asserted without
    /// depending on a real fs tool.
    struct FakeTool {
        name: &'static str,
        read_only: bool,
    }

    #[async_trait::async_trait(?Send)]
    impl Tool for FakeTool {
        fn name(&self) -> &'static str {
            self.name
        }
        fn schema(&self) -> Value {
            function_schema(self.name, "fake", json!({"type": "object"}))
        }
        fn command_line(&self, _sandbox: &dyn Sandbox, _call: &ToolCall) -> Option<String> {
            Some(self.name.to_string())
        }
        fn confirmation(&self, _sandbox: &dyn Sandbox, _call: &ToolCall) -> Option<Confirmation> {
            None
        }
        async fn execute(&self, _sandbox: &dyn Sandbox, _call: &ToolCall) -> ToolOutcome {
            ToolOutcome::Ok(format!("ran {}", self.name))
        }
        fn is_read_only(&self) -> bool {
            self.read_only
        }
    }

    /// Replays one pre-canned turn, driving the nested loop without a network.
    struct ScriptedProvider {
        turn: Mutex<Option<Result<CompletedTurn, AgentError>>>,
    }

    impl ScriptedProvider {
        fn ok(text: &str) -> Self {
            Self {
                turn: Mutex::new(Some(Ok(CompletedTurn {
                    content: text.to_string(),
                    tool_calls: Vec::new(),
                    thinking: None,
                }))),
            }
        }
        fn err() -> Self {
            Self {
                turn: Mutex::new(Some(Err(AgentError::Provider("boom".to_string())))),
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl CompletionProvider for ScriptedProvider {
        async fn complete(
            &self,
            _request: TurnRequest<'_>,
            _sink: &mut dyn EventSink,
        ) -> Result<CompletedTurn, AgentError> {
            self.turn.lock().unwrap().take().expect("one scripted turn")
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
                name: "task".to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn profile(id: &str, allowed_tools: Vec<&str>) -> AgentProfile {
        AgentProfile {
            id: id.to_string(),
            name: id.to_string(),
            description: format!("{id} test agent."),
            system_prompt: "You are a test agent.".to_string(),
            layer: Layer::Bundled,
            path: format!("<bundled>/agents/{id}.md"),
            model: None,
            allowed_tools: allowed_tools.into_iter().map(String::from).collect(),
        }
    }

    fn tool_pool() -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(FakeTool {
                name: "read_file",
                read_only: true,
            }),
            Arc::new(FakeTool {
                name: "write_file",
                read_only: false,
            }),
        ]
    }

    fn task_tool(
        provider: Arc<dyn CompletionProvider>,
        agents: HashMap<String, AgentProfile>,
    ) -> TaskTool {
        TaskTool::new(
            provider,
            tool_pool(),
            Arc::new(agents),
            "test-model".to_string(),
            Duration::from_secs(60),
            50,
        )
    }

    #[tokio::test]
    async fn filters_to_the_profiles_allowed_tools() {
        let tool = task_tool(
            Arc::new(ScriptedProvider::ok("done")),
            HashMap::from([(
                "searcher".to_string(),
                profile("searcher", vec!["read_file"]),
            )]),
        );
        let filtered = tool.tools_for(tool.agents.get("searcher").unwrap());
        let names: Vec<&str> = filtered.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["read_file"]);
    }

    #[tokio::test]
    async fn read_only_intersection_drops_a_disallowed_write_tool_even_if_named() {
        // A profile that lists write_file never gets it: v1 subagents are read-only regardless of what a
        // compromised profile requests.
        let tool = task_tool(
            Arc::new(ScriptedProvider::ok("done")),
            HashMap::from([(
                "danger".to_string(),
                profile("danger", vec!["read_file", "write_file"]),
            )]),
        );
        let filtered = tool.tools_for(tool.agents.get("danger").unwrap());
        let names: Vec<&str> = filtered.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["read_file"]);
    }

    #[tokio::test]
    async fn structural_depth_cap_never_includes_task_itself() {
        let tool = task_tool(Arc::new(ScriptedProvider::ok("done")), HashMap::new());
        assert!(tool.child_tools.iter().all(|t| t.name() != TaskTool::NAME));
    }

    #[tokio::test]
    async fn empty_allowed_tools_means_every_read_only_tool() {
        let tool = task_tool(
            Arc::new(ScriptedProvider::ok("done")),
            HashMap::from([("open".to_string(), profile("open", vec![]))]),
        );
        let filtered = tool.tools_for(tool.agents.get("open").unwrap());
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name(), "read_file");
    }

    #[tokio::test]
    async fn returns_the_subagents_final_text_to_the_parent() {
        let tool = task_tool(
            Arc::new(ScriptedProvider::ok("the answer")),
            HashMap::from([(
                "searcher".to_string(),
                profile("searcher", vec!["read_file"]),
            )]),
        );
        let out = tool
            .execute(
                &sandbox(),
                &call(r#"{"agent":"searcher","prompt":"find x"}"#),
            )
            .await;
        assert_eq!(out, ToolOutcome::Ok("the answer".to_string()));
    }

    #[tokio::test]
    async fn unknown_agent_lists_what_is_loaded() {
        let tool = task_tool(Arc::new(ScriptedProvider::ok("done")), HashMap::new());
        let out = tool
            .execute(&sandbox(), &call(r#"{"agent":"ghost","prompt":"x"}"#))
            .await;
        match out {
            ToolOutcome::Error(message) => assert!(message.contains("unknown agent 'ghost'")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provider_failure_surfaces_as_an_error_not_a_panic() {
        let tool = task_tool(
            Arc::new(ScriptedProvider::err()),
            HashMap::from([(
                "searcher".to_string(),
                profile("searcher", vec!["read_file"]),
            )]),
        );
        let out = tool
            .execute(&sandbox(), &call(r#"{"agent":"searcher","prompt":"x"}"#))
            .await;
        match out {
            ToolOutcome::Error(message) => assert!(message.contains("subagent 'searcher' failed")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn headless_io_approves_only_default_accept_confirmations() {
        let mut io = HeadlessIo;
        let accept = Confirmation {
            prompt: "x".to_string(),
            default_accept: true,
        };
        let decline = Confirmation {
            prompt: "x".to_string(),
            default_accept: false,
        };
        assert_eq!(io.decide(&accept).await, Approval::Approved);
        assert_eq!(io.decide(&decline).await, Approval::Declined);
    }

    #[tokio::test]
    async fn headless_io_declines_the_runaway_checkpoint() {
        let mut io = HeadlessIo;
        assert_eq!(
            io.confirm_continue(CheckpointReason::CallCount { calls: 1 })
                .await,
            Approval::Declined
        );
    }

    #[test]
    fn is_read_only() {
        let tool = task_tool(Arc::new(ScriptedProvider::ok("done")), HashMap::new());
        assert!(Tool::is_read_only(&tool));
    }
}
