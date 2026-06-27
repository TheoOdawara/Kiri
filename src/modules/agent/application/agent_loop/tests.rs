
use super::*;
use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use crate::modules::agent::application::approval_policy::ApprovalPolicy;
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::application::tool_observer::ToolObserver;
use crate::modules::provider::application::completion_provider::EventSink;
use crate::modules::tools::application::tool::Confirmation;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::FsSandbox;
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::role::Role;
use crate::shared::kernel::stream_event::StreamEvent;
use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

use regex::Regex;

use tempfile::TempDir;

/// A provider that replays pre-canned turns, ignoring the request — drives the loop without a network.
struct ScriptedProvider {
    turns: Mutex<VecDeque<CompletedTurn>>,
}

#[async_trait::async_trait(?Send)]
impl CompletionProvider for ScriptedProvider {
    async fn complete(
        &self,
        _request: TurnRequest<'_>,
        _sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        Ok(self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted turn"))
    }
}

/// A UI that decides every call with a fixed `Approval` and renders nothing.
struct ScriptedIo(Approval);

impl EventSink for ScriptedIo {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

impl Presenter for ScriptedIo {
    fn begin_turn(&mut self) {}
    fn finish_turn(&mut self) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for ScriptedIo {
    async fn decide(&mut self, _confirmation: &Confirmation) -> Approval {
        self.0
    }
    async fn confirm_continue(&mut self, _reason: CheckpointReason) -> Approval {
        self.0
    }
}

impl ToolObserver for ScriptedIo {
    fn tool_started(&mut self, _call: &ToolCall, _command: &str) {}
    fn tool_finished(&mut self, _call: &ToolCall, _outcome: &ToolOutcome, _elapsed: Duration) {}
}

/// A UI that answers each confirmation from a queue and counts how many times it was asked, so a
/// test can prove that later calls in the turn ran without prompting.
struct CountingIo {
    decisions: VecDeque<Approval>,
    decide_calls: u32,
}

impl EventSink for CountingIo {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

impl Presenter for CountingIo {
    fn begin_turn(&mut self) {}
    fn finish_turn(&mut self) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for CountingIo {
    async fn decide(&mut self, _confirmation: &Confirmation) -> Approval {
        self.decide_calls += 1;
        self.decisions.pop_front().unwrap_or(Approval::Declined)
    }
    async fn confirm_continue(&mut self, _reason: CheckpointReason) -> Approval {
        Approval::Approved
    }
}

impl ToolObserver for CountingIo {
    fn tool_started(&mut self, _call: &ToolCall, _command: &str) {}
    fn tool_finished(&mut self, _call: &ToolCall, _outcome: &ToolOutcome, _elapsed: Duration) {}
}

/// A UI double that records the command of every `tool_started` and the outcome of every
/// `tool_finished`, so a test can prove the loop surfaces each call in any approval mode.
struct RecordingIo {
    decision: Approval,
    started: Vec<String>,
    finished: Vec<ToolOutcome>,
}

impl RecordingIo {
    fn new(decision: Approval) -> Self {
        Self {
            decision,
            started: Vec::new(),
            finished: Vec::new(),
        }
    }
}

impl EventSink for RecordingIo {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

impl Presenter for RecordingIo {
    fn begin_turn(&mut self) {}
    fn finish_turn(&mut self) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for RecordingIo {
    async fn decide(&mut self, _confirmation: &Confirmation) -> Approval {
        self.decision
    }
    async fn confirm_continue(&mut self, _reason: CheckpointReason) -> Approval {
        self.decision
    }
}

impl ToolObserver for RecordingIo {
    fn tool_started(&mut self, _call: &ToolCall, command: &str) {
        self.started.push(command.to_string());
    }
    fn tool_finished(&mut self, _call: &ToolCall, outcome: &ToolOutcome, _elapsed: Duration) {
        self.finished.push(outcome.clone());
    }
}

fn tool_call(name: &str, args: &str) -> ToolCall {
    tool_call_id(name, args, "c1")
}

fn tool_call_id(name: &str, args: &str, id: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        },
    }
}

fn agent_loop_with(turns: Vec<CompletedTurn>) -> AgentLoop {
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(turns)),
    });
    AgentLoop::new(
        provider,
        ToolRegistry::new(default_fs_tools(
            Arc::from(Vec::<Regex>::new()),
            Arc::from(Vec::<Regex>::new()),
            false,
        )),
        "model".to_string(),
        Duration::from_secs(3600),
        1000,
    )
}

#[tokio::test]
async fn run_drives_a_tool_turn_then_a_text_turn() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();

    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "reading".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);

    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = ScriptedIo(Approval::Approved);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    let roles: Vec<Role> = conversation.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            Role::System,
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Assistant
        ]
    );
    // The tool result fed back is the file contents; the final assistant turn is the text.
    assert_eq!(conversation.messages()[3].content.as_deref(), Some("hello"));
    assert_eq!(conversation.messages()[4].content.as_deref(), Some("done"));
}

#[tokio::test]
async fn run_aborts_when_the_user_ends_the_session_at_a_prompt() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![CompletedTurn {
        content: String::new(),
        tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
    }]);

    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = ScriptedIo(Approval::Aborted);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();
    assert_eq!(outcome, TurnOutcome::Aborted);
}

#[tokio::test]
async fn auto_mode_runs_tools_without_asking() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call(
                "write_file",
                r#"{"path":"a.txt","content":"hi"}"#,
            )],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    // Would decline if asked — auto mode must not ask.
    let mut io = ScriptedIo(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hi"
    );
}

#[tokio::test]
async fn approved_auto_stops_asking_for_the_rest_of_the_turn() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    // One assistant turn with two destructive calls: the first prompts, the second must not.
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![
                tool_call_id("write_file", r#"{"path":"a.txt","content":"a"}"#, "c1"),
                tool_call_id("write_file", r#"{"path":"b.txt","content":"b"}"#, "c2"),
            ],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write two files"));
    let mut io = CountingIo {
        decisions: VecDeque::from(vec![Approval::ApprovedAuto]),
        decide_calls: 0,
    };

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(
        io.decide_calls, 1,
        "the second call must run under auto without asking again"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "a"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "b"
    );
}

#[tokio::test]
async fn plan_mode_blocks_destructive_tools() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call(
                "write_file",
                r#"{"path":"a.txt","content":"hi"}"#,
            )],
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    let mut io = ScriptedIo(Approval::Approved);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert!(
        !dir.path().join("a.txt").exists(),
        "plan mode must not write"
    );
    let tool_msg = conversation
        .messages()
        .iter()
        .find(|m| m.role == Role::Tool)
        .unwrap();
    assert!(
        tool_msg
            .content
            .as_deref()
            .unwrap()
            .contains("blocked in plan mode")
    );
}

#[tokio::test]
async fn plan_mode_allows_read_only_tools() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "reading".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    // Would decline if asked — plan mode runs read-only tools directly.
    let mut io = ScriptedIo(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    let tool_msg = conversation
        .messages()
        .iter()
        .find(|m| m.role == Role::Tool)
        .unwrap();
    assert_eq!(tool_msg.content.as_deref(), Some("hello"));
}

#[tokio::test]
async fn plan_mode_present_plan_proposes_the_plan_and_keeps_the_wire_valid() {
    // The explicit plan signal: a `present_plan` call in plan mode ends the turn as `PlanProposed`
    // (no execution), and the conversation stays a valid tool round (assistant tool_call answered
    // by a tool result) so the next turn after approval is accepted by the provider.
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![CompletedTurn {
        content: "vou planejar".to_string(),
        tool_calls: vec![tool_call("present_plan", r#"{"plan":"Plano final"}"#)],
    }]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("faça um plano"));
    // Would decline if asked — present_plan never goes through the confirmation flow.
    let mut io = ScriptedIo(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(
        outcome,
        TurnOutcome::PlanProposed("Plano final".to_string())
    );
    let roles: Vec<Role> = conversation.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::System, Role::User, Role::Assistant, Role::Tool],
        "the present_plan tool_call must be answered by a tool result"
    );
    assert!(
        conversation
            .messages()
            .last()
            .unwrap()
            .content
            .as_deref()
            .unwrap()
            .contains("apresentado")
    );
}

#[tokio::test]
async fn auto_mode_emits_tool_started_and_finished() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call(
                "write_file",
                r#"{"path":"a.txt","content":"hi"}"#,
            )],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    // Would decline if asked — auto must not ask, and must still surface the call.
    let mut io = RecordingIo::new(Approval::Declined);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(io.started, vec!["write a.txt".to_string()]);
    assert!(matches!(io.finished.as_slice(), [ToolOutcome::Ok(_)]));
}

#[tokio::test]
async fn default_mode_emits_around_execution() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "reading".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = RecordingIo::new(Approval::Approved);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();

    assert_eq!(io.started, vec!["cat a.txt".to_string()]);
    assert!(matches!(io.finished.as_slice(), [ToolOutcome::Ok(_)]));
}

#[tokio::test]
async fn plan_block_emits_started_and_error_finish() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call(
                "write_file",
                r#"{"path":"a.txt","content":"hi"}"#,
            )],
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    let mut io = RecordingIo::new(Approval::Approved);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(io.started, vec!["write a.txt".to_string()]);
    match io.finished.as_slice() {
        [ToolOutcome::Error(msg)] => assert!(msg.contains("blocked in plan mode")),
        other => panic!("expected a single Error finish, got {other:?}"),
    }
    assert!(
        !dir.path().join("a.txt").exists(),
        "plan mode must not write"
    );
}

#[tokio::test]
async fn declined_emits_started_and_declined_finish() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "deleting".to_string(),
            tool_calls: vec![tool_call("delete_file", r#"{"path":"a.txt"}"#)],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("delete a.txt"));
    let mut io = RecordingIo::new(Approval::Declined);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();

    assert_eq!(io.started, vec!["rm a.txt".to_string()]);
    assert!(matches!(io.finished.as_slice(), [ToolOutcome::Declined]));
    assert!(
        dir.path().join("a.txt").exists(),
        "declined must not delete"
    );
}

#[tokio::test]
async fn auto_mode_runs_inroot_read_without_confirming() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "reading".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = CountingIo {
        decisions: VecDeque::new(),
        decide_calls: 0,
    };

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(
        io.decide_calls, 0,
        "an in-root read must not prompt in auto mode"
    );
}

#[tokio::test]
async fn auto_mode_confirms_destructive_delete() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "deleting".to_string(),
            tool_calls: vec![tool_call("delete_file", r#"{"path":"a.txt"}"#)],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("delete a.txt"));
    // Declines the destructive call — auto must still ask, so the file survives.
    let mut io = CountingIo {
        decisions: VecDeque::from(vec![Approval::Declined]),
        decide_calls: 0,
    };

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(
        io.decide_calls, 1,
        "a destructive tool must be confirmed even in auto mode"
    );
    assert!(
        dir.path().join("a.txt").exists(),
        "a declined delete must not run, even in auto mode"
    );
}

#[tokio::test]
async fn auto_mode_confirms_out_of_root_target() {
    let outside = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let target = outside.path().join("new.txt");
    let args = serde_json::json!({ "path": target.to_str().unwrap(), "content": "x" }).to_string();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call("write_file", &args)],
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write outside the workspace"));
    // write_file is an ordinary mutation, but the target is outside the root — auto must still ask.
    let mut io = CountingIo {
        decisions: VecDeque::from(vec![Approval::Declined]),
        decide_calls: 0,
    };

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(
        io.decide_calls, 1,
        "an out-of-root target must be confirmed even in auto mode"
    );
    assert!(
        !target.exists(),
        "a declined out-of-root write must not run, even in auto mode"
    );
}

#[tokio::test]
async fn iteration_cap_fires_the_checkpoint() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    // Two read-only rounds are queued; with a cap of 1 the checkpoint must fire after the first.
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![
            CompletedTurn {
                content: "first".to_string(),
                tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
            },
            CompletedTurn {
                content: "second".to_string(),
                tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
            },
        ])),
    });
    let agent_loop = AgentLoop::new(
        provider,
        ToolRegistry::new(default_fs_tools(
            Arc::from(Vec::<Regex>::new()),
            Arc::from(Vec::<Regex>::new()),
            false,
        )),
        "model".to_string(),
        Duration::from_secs(3600),
        1,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read repeatedly"));
    // Declining the checkpoint ends the turn before the second round can run.
    let mut io = ScriptedIo(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    let tool_results = conversation
        .messages()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .count();
    assert_eq!(
        tool_results, 1,
        "the call cap must pause the turn before the second tool round"
    );
}

fn registry_for_tests() -> ToolRegistry {
    ToolRegistry::new(default_fs_tools(
        Arc::from(Vec::<Regex>::new()),
        Arc::from(Vec::<Regex>::new()),
        false,
    ))
}

#[tokio::test]
async fn the_call_cap_pauses_within_a_single_oversized_round() {
    // One assistant message carrying three write_file calls, cap 2, in auto. The cap must trip
    // WITHIN the round: the first two write, the third is paused-and-declined before executing —
    // the regression where a single round could run an unbounded burst before any checkpoint.
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![CompletedTurn {
            content: String::new(),
            tool_calls: vec![
                tool_call_id("write_file", r#"{"path":"a.txt","content":"x"}"#, "c0"),
                tool_call_id("write_file", r#"{"path":"b.txt","content":"x"}"#, "c1"),
                tool_call_id("write_file", r#"{"path":"c.txt","content":"x"}"#, "c2"),
            ],
        }])),
    });
    let agent_loop = AgentLoop::new(
        provider,
        registry_for_tests(),
        "m".to_string(),
        Duration::from_secs(3600),
        2,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write three"));
    let mut io = ScriptedIo(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert!(dir.path().join("a.txt").exists());
    assert!(dir.path().join("b.txt").exists());
    assert!(
        !dir.path().join("c.txt").exists(),
        "the 3rd call must not run once the cap trips mid-round"
    );
    // Every call is still answered, so the exchange stays valid.
    let tool_results = conversation
        .messages()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .count();
    assert_eq!(tool_results, 3);
}

#[tokio::test]
async fn aborting_mid_round_answers_every_tool_call() {
    // The user aborts at the first of two calls; both must still receive a tool_result so the
    // assistant tool_calls message is a fully-answered (valid, persistable) exchange.
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![CompletedTurn {
            content: String::new(),
            tool_calls: vec![
                tool_call_id("delete_file", r#"{"path":"a.txt"}"#, "c0"),
                tool_call_id("delete_file", r#"{"path":"b.txt"}"#, "c1"),
            ],
        }])),
    });
    let agent_loop = AgentLoop::new(
        provider,
        registry_for_tests(),
        "m".to_string(),
        Duration::from_secs(3600),
        100,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("delete two"));
    let mut io = ScriptedIo(Approval::Aborted);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Aborted);
    let tool_results = conversation
        .messages()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .count();
    assert_eq!(
        tool_results, 2,
        "an aborted round must still answer every tool_call"
    );
}

/// A provider that always fails, to exercise the error path.
struct FailingProvider;
#[async_trait::async_trait(?Send)]
impl CompletionProvider for FailingProvider {
    async fn complete(
        &self,
        _request: TurnRequest<'_>,
        _sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        Err(AgentError::Provider("boom".to_string()))
    }
}

/// A UI double that counts `finish_turn` calls (and auto-approves everything else).
struct FinishCountingIo {
    finishes: u32,
}
impl EventSink for FinishCountingIo {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}
impl Presenter for FinishCountingIo {
    fn begin_turn(&mut self) {}
    fn finish_turn(&mut self) -> Result<(), AgentError> {
        self.finishes += 1;
        Ok(())
    }
}
#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for FinishCountingIo {
    async fn decide(&mut self, _confirmation: &Confirmation) -> Approval {
        Approval::Approved
    }
    async fn confirm_continue(&mut self, _reason: CheckpointReason) -> Approval {
        Approval::Approved
    }
}
impl ToolObserver for FinishCountingIo {
    fn tool_started(&mut self, _call: &ToolCall, _command: &str) {}
    fn tool_finished(&mut self, _call: &ToolCall, _outcome: &ToolOutcome, _elapsed: Duration) {}
}

#[tokio::test]
async fn provider_failure_propagates_after_finishing_the_render() {
    // The contract requires render cleanup (spinner erase / terminal reset) on the failure path too,
    // and the error to propagate. A refactor moving finish_turn after `?` would pass every other
    // test but break this.
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = AgentLoop::new(
        Arc::new(FailingProvider),
        registry_for_tests(),
        "m".to_string(),
        Duration::from_secs(3600),
        100,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("hi"));
    let mut io = FinishCountingIo { finishes: 0 };

    let result = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await;

    assert!(
        matches!(result, Err(AgentError::Provider(_))),
        "the provider error must propagate"
    );
    assert_eq!(
        io.finishes, 1,
        "finish_turn must run exactly once before the error propagates"
    );
}

/// A UI double that records every checkpoint reason it is shown and answers with a fixed decision.
struct ReasonRecordingIo {
    reasons: Vec<CheckpointReason>,
    decision: Approval,
}
impl EventSink for ReasonRecordingIo {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}
impl Presenter for ReasonRecordingIo {
    fn begin_turn(&mut self) {}
    fn finish_turn(&mut self) -> Result<(), AgentError> {
        Ok(())
    }
}
#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for ReasonRecordingIo {
    async fn decide(&mut self, _confirmation: &Confirmation) -> Approval {
        self.decision
    }
    async fn confirm_continue(&mut self, reason: CheckpointReason) -> Approval {
        self.reasons.push(reason);
        self.decision
    }
}
impl ToolObserver for ReasonRecordingIo {
    fn tool_started(&mut self, _call: &ToolCall, _command: &str) {}
    fn tool_finished(&mut self, _call: &ToolCall, _outcome: &ToolOutcome, _elapsed: Duration) {}
}

#[tokio::test]
async fn wall_clock_checkpoint_fires_with_an_elapsed_reason() {
    // A zero wall-clock budget trips the time leg after the first round; the cap is large so it is
    // NOT the call-count leg — the reason shown must be Elapsed, not CallCount.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![CompletedTurn {
            content: "x".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
        }])),
    });
    let agent_loop = AgentLoop::new(
        provider,
        registry_for_tests(),
        "m".to_string(),
        Duration::ZERO,
        100,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read"));
    let mut io = ReasonRecordingIo {
        reasons: Vec::new(),
        decision: Approval::Declined,
    };

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(io.reasons.len(), 1);
    assert!(
        matches!(io.reasons[0], CheckpointReason::Elapsed { .. }),
        "a zero time budget must trip the wall-clock leg, got {:?}",
        io.reasons[0]
    );
}

/// The production seam the other tests skip: drive a turn through the REAL `Bridge` adapter (not the
/// scripted IO double) and assert the engine emits `Began` first — the message that flips the
/// spinner / streaming on. A regression here is exactly "first message does nothing, no spinner".
#[tokio::test]
async fn run_through_the_real_bridge_emits_began_first_then_content() {
    use crate::modules::tui::infrastructure::bridge::{Bridge, CancelToken, EngineMsg};
    use crate::shared::kernel::stream_event::StreamEvent;
    use tokio::sync::mpsc;

    // A provider that streams one content delta through the sink, then finishes (no tools).
    struct EmittingProvider;
    #[async_trait::async_trait(?Send)]
    impl CompletionProvider for EmittingProvider {
        async fn complete(
            &self,
            _request: TurnRequest<'_>,
            sink: &mut dyn EventSink,
        ) -> Result<CompletedTurn, AgentError> {
            sink.on_event(StreamEvent::Content("hi".to_string()))?;
            Ok(CompletedTurn {
                content: "hi".to_string(),
                tool_calls: vec![],
            })
        }
    }

    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = AgentLoop::new(
        Arc::new(EmittingProvider),
        ToolRegistry::new(default_fs_tools(
            Arc::from(Vec::<Regex>::new()),
            Arc::from(Vec::<Regex>::new()),
            false,
        )),
        "model".to_string(),
        Duration::from_secs(3600),
        1000,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("hi"));

    let (tx, mut rx) = mpsc::unbounded_channel::<EngineMsg>();
    let mut bridge = Bridge::new(tx, CancelToken::new());

    let outcome = agent_loop
        .run(
            &mut conversation,
            &sandbox,
            ApprovalMode::Default,
            &mut bridge,
        )
        .await
        .unwrap();
    assert_eq!(outcome, TurnOutcome::Completed);

    let mut msgs = Vec::new();
    while let Ok(m) = rx.try_recv() {
        msgs.push(m);
    }
    assert!(
        matches!(msgs.first(), Some(EngineMsg::Began)),
        "the first engine message must be Began (the spinner / streaming trigger)"
    );
    assert!(
        msgs.iter()
            .any(|m| matches!(m, EngineMsg::Content(t) if t == "hi")),
        "the streamed content delta must be forwarded to the runtime"
    );
    assert!(
        msgs.iter().any(|m| matches!(m, EngineMsg::Finished)),
        "the turn must signal Finished"
    );
}
