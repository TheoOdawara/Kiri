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
use crate::shared::kernel::message::ThinkingBlock;
use crate::shared::kernel::role::Role;
use crate::shared::kernel::stream_event::StreamEvent;
use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

use tempfile::TempDir;

#[derive(Debug)]
struct TestConfinement;

impl CommandSandbox for TestConfinement {
    fn confine(
        &self,
        command: tokio::process::Command,
        _policy: &SandboxPolicy,
    ) -> Result<tokio::process::Command, AgentError> {
        Ok(command)
    }

    fn guarantees(&self) -> crate::modules::tools::application::command_sandbox::SandboxGuarantees {
        crate::modules::tools::application::command_sandbox::SandboxGuarantees::BWRAP
    }
}

struct ScriptedReviewer {
    decision: ReviewDecision,
    requests: Mutex<Vec<ActionReviewRequest>>,
}

#[async_trait::async_trait(?Send)]
impl ActionReviewer for ScriptedReviewer {
    async fn review(
        &self,
        _provider: &dyn CompletionProvider,
        _model: &str,
        request: &ActionReviewRequest,
    ) -> Result<crate::modules::agent::application::action_reviewer::ActionReview, AgentError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(
            crate::modules::agent::application::action_reviewer::ActionReview {
                decision: self.decision,
                reason: "scripted decision".to_string(),
            },
        )
    }
}

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

/// One configurable UI double for every agent-loop test. Answers each confirmation from the `decisions`
/// queue in order, then falls back to `default_decision`, recording everything it was shown.
struct TestIo {
    decisions: VecDeque<Approval>,
    default_decision: Approval,
    decide_calls: u32,
    finishes: u32,
    started: Vec<String>,
    finished: Vec<ToolOutcome>,
    reasons: Vec<CheckpointReason>,
}

impl TestIo {
    /// Answer every confirmation with `default_decision` until a queued answer overrides it.
    fn new(default_decision: Approval) -> Self {
        Self {
            decisions: VecDeque::new(),
            default_decision,
            decide_calls: 0,
            finishes: 0,
            started: Vec::new(),
            finished: Vec::new(),
            reasons: Vec::new(),
        }
    }

    /// Queue the leading confirmation answers, consumed in order before `default_decision` takes over.
    fn with_decisions(mut self, decisions: Vec<Approval>) -> Self {
        self.decisions = VecDeque::from(decisions);
        self
    }
}

impl EventSink for TestIo {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

impl Presenter for TestIo {
    fn begin_round(&mut self) {}
    fn finish_round(&mut self) -> Result<(), AgentError> {
        self.finishes += 1;
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for TestIo {
    async fn decide(&mut self, _confirmation: &Confirmation) -> Approval {
        self.decide_calls += 1;
        self.decisions.pop_front().unwrap_or(self.default_decision)
    }
    async fn confirm_continue(&mut self, reason: CheckpointReason) -> Approval {
        self.reasons.push(reason);
        self.decisions.pop_front().unwrap_or(self.default_decision)
    }
}

impl ToolObserver for TestIo {
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

/// The full fs tool set with no sensitive-path matchers, the single construction every test shares.
fn registry_for_tests() -> ToolRegistry {
    // The default command policy admits `echo` in plan mode, so the SEC-01 test reaches the confirmation
    // gate; destructive tools are blocked by being non-plannable, independent of the policy.
    ToolRegistry::new(default_fs_tools(Arc::default(), false))
}

#[test]
fn plan_workspace_view_forces_read_only_command_policy() {
    let dir = TempDir::new().unwrap();
    let configured_write = dir.path().join("configured-cache");
    let sandbox = FsSandbox::with_confinement(
        dir.path(),
        SensitiveMatcher::empty(),
        Arc::new(TestConfinement),
        crate::shared::kernel::sandbox::NetworkPolicy::Deny,
        Arc::from(Vec::new()),
        Arc::from(vec![configured_write.clone()]),
    )
    .unwrap();
    let view = ReadOnlyWorkspace(&sandbox);
    let command_cwd = dir.path().join("nested");

    let policy = view.command_policy(
        crate::shared::kernel::sandbox::NetworkPolicy::Deny,
        &[],
        &[&command_cwd],
    );

    assert_eq!(
        policy.workspace_access,
        crate::modules::tools::application::command_sandbox::WorkspaceAccess::ReadOnly
    );
    assert!(policy.extra_rw.is_empty());
    assert_eq!(policy.extra_ro, vec![configured_write, command_cwd]);
}

fn agent_loop_with(turns: Vec<CompletedTurn>) -> AgentLoop {
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(turns)),
    });
    AgentLoop::new(
        provider,
        registry_for_tests(),
        "model".to_string(),
        Duration::from_secs(3600),
        1000,
    )
}

fn agent_loop_with_reviewer(
    turns: Vec<CompletedTurn>,
    reviewer: Arc<dyn ActionReviewer>,
) -> AgentLoop {
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(turns)),
    });
    AgentLoop::new(
        provider,
        registry_for_tests(),
        "model".to_string(),
        Duration::from_secs(3600),
        1000,
    )
    .with_reviewer(reviewer)
}

fn confined_sandbox(dir: &TempDir) -> FsSandbox {
    FsSandbox::with_confinement(
        dir.path(),
        SensitiveMatcher::empty(),
        Arc::new(TestConfinement),
        crate::shared::kernel::sandbox::NetworkPolicy::Deny,
        Arc::from(Vec::new()),
        Arc::from(Vec::new()),
    )
    .unwrap()
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
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);

    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = TestIo::new(Approval::Approved);

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
async fn thinking_attaches_to_the_pushed_message_on_a_plain_text_turn() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![CompletedTurn {
        content: "done".to_string(),
        tool_calls: vec![],
        thinking: Some(ThinkingBlock::Visible {
            text: "reasoning".to_string(),
            signature: Some("sig".to_string()),
        }),
    }]);

    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("hi"));
    let mut io = TestIo::new(Approval::Approved);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();

    let pushed = conversation.messages().last().unwrap();
    match pushed.thinking.as_ref().expect("thinking must be attached") {
        ThinkingBlock::Visible { text, signature } => {
            assert_eq!(text, "reasoning");
            assert_eq!(signature.as_deref(), Some("sig"));
        }
        other => panic!("expected Visible, got {other:?}"),
    }
}

#[tokio::test]
async fn thinking_attaches_to_the_pushed_message_on_a_tool_call_turn() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "reading".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
            thinking: Some(ThinkingBlock::Redacted {
                data: "encrypted-blob".to_string(),
            }),
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);

    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = TestIo::new(Approval::Approved);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();

    // System, User, Assistant(tool call + thinking), Tool, Assistant(text, no thinking)
    let tool_call_message = &conversation.messages()[2];
    match tool_call_message
        .thinking
        .as_ref()
        .expect("thinking must be attached to the tool-call turn")
    {
        ThinkingBlock::Redacted { data } => assert_eq!(data, "encrypted-blob"),
        other => panic!("expected Redacted, got {other:?}"),
    }
}

#[tokio::test]
async fn run_aborts_when_the_user_ends_the_session_at_a_prompt() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![CompletedTurn {
        content: String::new(),
        tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
        thinking: None,
    }]);

    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = TestIo::new(Approval::Aborted);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await
        .unwrap();
    assert_eq!(outcome, TurnOutcome::Aborted);
}

#[tokio::test]
async fn auto_mode_runs_tools_without_asking() {
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call(
                "write_file",
                r#"{"path":"a.txt","content":"hi"}"#,
            )],
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    // Would decline if asked — auto mode must not ask.
    let mut io = TestIo::new(Approval::Declined);

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
    let sandbox = confined_sandbox(&dir);
    // One assistant turn with two destructive calls: the first prompts, the second must not.
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![
                tool_call_id("write_file", r#"{"path":"a.txt","content":"a"}"#, "c1"),
                tool_call_id("write_file", r#"{"path":"b.txt","content":"b"}"#, "c2"),
            ],
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write two files"));
    let mut io = TestIo::new(Approval::Declined).with_decisions(vec![Approval::ApprovedAuto]);

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
            thinking: None,
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    let mut io = TestIo::new(Approval::Approved);

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
            thinking: None,
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    // Would decline if asked — plan mode runs read-only tools directly.
    let mut io = TestIo::new(Approval::Declined);

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
async fn auto_mode_runs_a_shell_command_only_after_automated_review() {
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    let reviewer = Arc::new(ScriptedReviewer {
        decision: ReviewDecision::Allow,
        requests: Mutex::new(Vec::new()),
    });
    let agent_loop = agent_loop_with_reviewer(
        vec![
            CompletedTurn {
                content: "looking".to_string(),
                tool_calls: vec![tool_call("run_command", r#"{"command":"echo hello"}"#)],
                thinking: None,
            },
            CompletedTurn {
                content: "done".to_string(),
                tool_calls: vec![],
                thinking: None,
            },
        ],
        reviewer.clone(),
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("work unattended"));
    let mut io = TestIo::new(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(
        io.decide_calls, 0,
        "auto mode never interrupts the user for an action decision"
    );
    let first_result = conversation
        .messages()
        .iter()
        .find(|message| message.role == Role::Tool)
        .and_then(|message| message.content.clone())
        .unwrap_or_default();
    assert!(
        first_result.contains("hello"),
        "the reviewed command must really have executed: {first_result}"
    );
    let requests = reviewer.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].tool, "run_command");
}

#[tokio::test]
async fn plan_mode_runs_an_allowlisted_command_without_confirmation() {
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "running".to_string(),
            tool_calls: vec![tool_call("run_command", r#"{"command":"echo hi"}"#)],
            thinking: None,
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("run a command while planning"));
    let mut io = TestIo::new(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(
        io.decide_calls, 0,
        "allow-listed inspection/build/test commands run freely in plan mode"
    );
    assert!(matches!(io.finished.as_slice(), [ToolOutcome::Ok(output)] if output.contains("hi")));
}

#[tokio::test]
async fn plan_mode_refuses_commands_when_read_only_confinement_is_off() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "checking".to_string(),
            tool_calls: vec![tool_call("run_command", r#"{"command":"echo hi"}"#)],
            thinking: None,
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("inspect without a sandbox"));
    let mut io = TestIo::new(Approval::Approved);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(io.decide_calls, 0);
    assert!(matches!(io.finished.as_slice(), [ToolOutcome::Error(_)]));
}

#[tokio::test]
async fn run_command_stays_mutating_even_when_plan_executes_it_read_only() {
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let registry = registry_for_tests();
    let call = tool_call("run_command", r#"{"command":"echo hi"}"#);
    let confirmation = registry.confirm(&sandbox, &call).expect("parsed args");
    assert!(
        registry.confirm_in_auto(&call, &confirmation),
        "every shell command requires automated review in auto mode"
    );
    assert!(
        registry.is_destructive("run_command"),
        "the tool can still mutate outside plan mode"
    );
}

#[tokio::test]
async fn checkpoint_approved_auto_does_not_reopen_the_plan_blacklist() {
    // A turn that starts in Plan and escalates to Auto mid-turn must still refuse a plan_check-blacklisted
    // run_command, never downgrade it to "needs a live confirmation". max_tool_calls=1 forces the
    // checkpoint after round 1's single allowed call.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("marker.txt"), b"still here").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = AgentLoop::new(
        Arc::new(ScriptedProvider {
            turns: Mutex::new(VecDeque::from(vec![
                // Round 1: an allow-listed command (echo), free in Plan — the call cap then trips.
                CompletedTurn {
                    content: "planning".to_string(),
                    tool_calls: vec![tool_call("run_command", r#"{"command":"echo hi"}"#)],
                    thinking: None,
                },
                // Round 2, now mode == Auto (via the checkpoint's ApprovedAuto below): a command NOT in
                // the plan-mode allow-list — must still be refused, not confirm-prompted.
                CompletedTurn {
                    content: "escalated".to_string(),
                    tool_calls: vec![tool_call("run_command", r#"{"command":"rm marker.txt"}"#)],
                    thinking: None,
                },
                CompletedTurn {
                    content: "done".to_string(),
                    tool_calls: vec![],
                    thinking: None,
                },
            ])),
        }),
        registry_for_tests(),
        "model".to_string(),
        Duration::from_secs(3600),
        1,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("plan something"));
    // The only decision is the post-round checkpoint; the allowed Plan command does not prompt.
    let mut io = TestIo::new(Approval::Approved).with_decisions(vec![Approval::ApprovedAuto]);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert!(
        dir.path().join("marker.txt").exists(),
        "the blacklisted command must never execute, even after the checkpoint escalates to Auto"
    );
    assert_eq!(
        io.decide_calls, 0,
        "the blocked round-2 call must be refused outright, never reaching a confirmation prompt"
    );
    let last_tool_msg = conversation
        .messages()
        .iter()
        .rfind(|m| m.role == Role::Tool)
        .unwrap();
    assert!(
        last_tool_msg
            .content
            .as_deref()
            .unwrap()
            .contains("blocked in plan mode"),
        "got: {:?}",
        last_tool_msg.content
    );
}

#[tokio::test]
async fn checkpoint_approved_auto_reviews_an_admitted_plan_command() {
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    let reviewer = Arc::new(ScriptedReviewer {
        decision: ReviewDecision::Allow,
        requests: Mutex::new(Vec::new()),
    });
    let agent_loop = AgentLoop::new(
        Arc::new(ScriptedProvider {
            turns: Mutex::new(VecDeque::from(vec![
                CompletedTurn {
                    content: "planning".to_string(),
                    tool_calls: vec![tool_call("run_command", r#"{"command":"echo first"}"#)],
                    thinking: None,
                },
                CompletedTurn {
                    content: "continuing".to_string(),
                    tool_calls: vec![tool_call("run_command", r#"{"command":"echo second"}"#)],
                    thinking: None,
                },
                CompletedTurn {
                    content: "done".to_string(),
                    tool_calls: vec![],
                    thinking: None,
                },
            ])),
        }),
        registry_for_tests(),
        "model".to_string(),
        Duration::from_secs(3600),
        1,
    )
    .with_reviewer(reviewer.clone());
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("plan something"));
    let mut io = TestIo::new(Approval::Approved).with_decisions(vec![Approval::ApprovedAuto]);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(io.decide_calls, 0, "the action path must stay prompt-free");
    let requests = reviewer.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "the post-checkpoint command must be reviewed"
    );
    assert!(requests[0].action.contains("echo second"));
}

#[tokio::test]
async fn checkpoint_approved_auto_does_not_defeat_present_plan_interception() {
    // The sibling test above locks run_command's blacklist across a mid-turn ApprovedAuto escalation; this
    // one locks the `present_plan` interception across the same escalation. Gating it on the live `mode`
    // instead of `started_in_plan` makes `present_plan` execute as an ordinary tool, echoing the plan back
    // instead of ending the turn with `TurnOutcome::PlanProposed`.
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = AgentLoop::new(
        Arc::new(ScriptedProvider {
            turns: Mutex::new(VecDeque::from(vec![
                // run_command, not a read-only call: it consumes a `decide` confirmation, so the
                // post-round checkpoint consumes the SECOND queued decision (ApprovedAuto) and genuinely
                // flips `mode` to Auto. A plannable read-only call needs no confirmation, would let the
                // checkpoint consume the wrong decision, and would make this test pass vacuously.
                CompletedTurn {
                    content: "investigating".to_string(),
                    tool_calls: vec![tool_call("run_command", r#"{"command":"echo hi"}"#)],
                    thinking: None,
                },
                // Round 2, now mode == Auto (via the checkpoint's ApprovedAuto below): present_plan
                // must still end the turn as PlanProposed, never execute as an ordinary tool.
                CompletedTurn {
                    content: "escalated".to_string(),
                    tool_calls: vec![tool_call("present_plan", r#"{"plan":"Plano final"}"#)],
                    thinking: None,
                },
            ])),
        }),
        registry_for_tests(),
        "model".to_string(),
        Duration::from_secs(3600),
        1,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("plan something"));
    // Consumed in order: round 1's per-call confirmation, then the post-round checkpoint's decision.
    let mut io = TestIo::new(Approval::Approved)
        .with_decisions(vec![Approval::Approved, Approval::ApprovedAuto]);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(
        outcome,
        TurnOutcome::PlanProposed("Plano final".to_string()),
        "present_plan must still end the turn as PlanProposed after a mid-turn ApprovedAuto escalation"
    );
}

#[tokio::test]
async fn plan_mode_refuses_out_of_root_read_without_human_confirmation() {
    // Plan is prompt-free and workspace-scoped: a prompt-injected turn cannot exfiltrate an external file.
    let outside = TempDir::new().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, b"top secret").unwrap();
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let args = serde_json::json!({ "path": secret.to_str().unwrap() }).to_string();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "reading".to_string(),
            tool_calls: vec![tool_call("read_file", &args)],
            thinking: None,
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read an out-of-root file while planning"));
    let mut io = TestIo::new(Approval::Declined);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Plan, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(io.decide_calls, 0, "plan mode never asks the user");
    let tool_msg = conversation
        .messages()
        .iter()
        .find(|m| m.role == Role::Tool)
        .unwrap();
    assert!(
        !tool_msg
            .content
            .as_deref()
            .unwrap_or("")
            .contains("top secret"),
        "a refused out-of-root read must not leak the file to the model"
    );
    assert!(matches!(io.finished.as_slice(), [ToolOutcome::Error(_)]));
}

#[tokio::test]
async fn plan_mode_present_plan_proposes_the_plan_and_keeps_the_wire_valid() {
    // `present_plan` ends the turn as `PlanProposed` with no execution, and the conversation stays a valid
    // tool round so the provider accepts the next turn after approval.
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = agent_loop_with(vec![CompletedTurn {
        content: "vou planejar".to_string(),
        tool_calls: vec![tool_call("present_plan", r#"{"plan":"Plano final"}"#)],
        thinking: None,
    }]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("faça um plano"));
    // Would decline if asked — present_plan never goes through the confirmation flow.
    let mut io = TestIo::new(Approval::Declined);

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
            .contains("presented")
    );
}

#[tokio::test]
async fn auto_mode_emits_tool_started_and_finished() {
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call(
                "write_file",
                r#"{"path":"a.txt","content":"hi"}"#,
            )],
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    // Would decline if asked — auto must not ask, and must still surface the call.
    let mut io = TestIo::new(Approval::Declined);

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
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = TestIo::new(Approval::Approved);

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
            thinking: None,
        },
        CompletedTurn {
            content: "plan".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write a.txt"));
    let mut io = TestIo::new(Approval::Approved);

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
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("delete a.txt"));
    let mut io = TestIo::new(Approval::Declined);

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
    let sandbox = confined_sandbox(&dir);
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "reading".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = TestIo::new(Approval::Declined);

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
async fn auto_mode_refuses_a_destructive_delete_without_human_confirmation() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
    let sandbox = confined_sandbox(&dir);
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "deleting".to_string(),
            tool_calls: vec![tool_call("delete_file", r#"{"path":"a.txt"}"#)],
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("delete a.txt"));
    // The default reviewer is fail-closed, so the file survives without a human prompt.
    let mut io = TestIo::new(Approval::Declined);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(
        io.decide_calls, 0,
        "auto must never use the human approval policy"
    );
    assert!(
        dir.path().join("a.txt").exists(),
        "a declined delete must not run, even in auto mode"
    );
}

#[tokio::test]
async fn auto_executes_a_risky_action_only_when_the_reviewer_allows_it() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
    let sandbox = confined_sandbox(&dir);
    let reviewer = Arc::new(ScriptedReviewer {
        decision: ReviewDecision::Allow,
        requests: Mutex::new(Vec::new()),
    });
    let agent_loop = agent_loop_with_reviewer(
        vec![
            CompletedTurn {
                content: "deleting".to_string(),
                tool_calls: vec![tool_call("delete_file", r#"{"path":"a.txt"}"#)],
                thinking: None,
            },
            CompletedTurn {
                content: "done".to_string(),
                tool_calls: vec![],
                thinking: None,
            },
        ],
        reviewer.clone(),
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("delete a.txt"));
    let mut io = TestIo::new(Approval::Declined);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(io.decide_calls, 0);
    assert!(!dir.path().join("a.txt").exists());
    let requests = reviewer.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].user_intent, "delete a.txt");
    assert_eq!(requests[0].tool, "delete_file");
}

#[tokio::test]
async fn auto_with_sandbox_off_reviews_even_a_safe_read() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let reviewer = Arc::new(ScriptedReviewer {
        decision: ReviewDecision::Allow,
        requests: Mutex::new(Vec::new()),
    });
    let agent_loop = agent_loop_with_reviewer(
        vec![
            CompletedTurn {
                content: "reading".to_string(),
                tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
                thinking: None,
            },
            CompletedTurn {
                content: "done".to_string(),
                tool_calls: vec![],
                thinking: None,
            },
        ],
        reviewer.clone(),
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read a.txt"));
    let mut io = TestIo::new(Approval::Declined);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(io.decide_calls, 0);
    assert_eq!(reviewer.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn auto_mode_refuses_an_out_of_root_target_without_human_confirmation() {
    let outside = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    let target = outside.path().join("new.txt");
    let args = serde_json::json!({ "path": target.to_str().unwrap(), "content": "x" }).to_string();
    let agent_loop = agent_loop_with(vec![
        CompletedTurn {
            content: "writing".to_string(),
            tool_calls: vec![tool_call("write_file", &args)],
            thinking: None,
        },
        CompletedTurn {
            content: "done".to_string(),
            tool_calls: vec![],
            thinking: None,
        },
    ]);
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("write outside the workspace"));
    // An out-of-root target is reviewer-gated and fails closed.
    let mut io = TestIo::new(Approval::Declined);

    agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(
        io.decide_calls, 0,
        "auto must never use the human approval policy"
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
    let sandbox = confined_sandbox(&dir);
    // Two read-only rounds are queued; with a cap of 1 the checkpoint must fire after the first.
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![
            CompletedTurn {
                content: "first".to_string(),
                tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
                thinking: None,
            },
            CompletedTurn {
                content: "second".to_string(),
                tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
                thinking: None,
            },
        ])),
    });
    let agent_loop = AgentLoop::new(
        provider,
        registry_for_tests(),
        "model".to_string(),
        Duration::from_secs(3600),
        1,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("read repeatedly"));
    // Declining the checkpoint ends the turn before the second round can run.
    let mut io = TestIo::new(Approval::Declined);

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

#[tokio::test]
async fn a_tool_timeout_is_never_auto_retried_within_the_turn() {
    // A mutating tool cannot cancel its underlying syscall once dispatched, so a reported timeout does not
    // prove the write stopped; an automatic retry could race the still-running first attempt. The
    // invariant that makes this safe: a timed-out call produces exactly ONE execution attempt and one
    // tool_result, and only the model's next turn can retry. run_command stands in for every tool here.
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    // `timeout_ms` is clamped to a 1000ms floor, so the 100ms requested below actually races a ~1s
    // timeout; a comfortably slower command keeps this deterministic.
    let slow = if cfg!(windows) {
        "ping -n 6 127.0.0.1 > nul"
    } else {
        "sleep 5"
    };
    let reviewer = Arc::new(ScriptedReviewer {
        decision: ReviewDecision::Allow,
        requests: Mutex::new(Vec::new()),
    });
    let agent_loop = agent_loop_with_reviewer(
        vec![
            CompletedTurn {
                content: "running".to_string(),
                tool_calls: vec![tool_call(
                    "run_command",
                    &format!(r#"{{"command":"{slow}","timeout_ms":100}}"#),
                )],
                thinking: None,
            },
            CompletedTurn {
                content: "done".to_string(),
                tool_calls: vec![],
                thinking: None,
            },
        ],
        reviewer,
    );
    let mut conversation = Conversation::new("system");
    conversation.push(Message::user("run something slow"));
    let mut io = TestIo::new(Approval::Approved);

    let outcome = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Auto, &mut io)
        .await
        .unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    let tool_results: Vec<_> = conversation
        .messages()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .collect();
    assert_eq!(
        tool_results.len(),
        1,
        "a timed-out call must be answered exactly once, never retried within the same round"
    );
    assert!(
        tool_results[0]
            .content
            .as_deref()
            .unwrap()
            .contains("timed out"),
        "got: {:?}",
        tool_results[0].content
    );
    assert_eq!(
        io.finished.len(),
        1,
        "exactly one execution attempt must reach tool_finished — no automatic retry"
    );
}

#[tokio::test]
async fn the_call_cap_pauses_within_a_single_oversized_round() {
    // Three write_file calls in one assistant message, cap 2, in auto. The cap must trip WITHIN the round:
    // the first two write, the third is paused-and-declined before executing.
    let dir = TempDir::new().unwrap();
    let sandbox = confined_sandbox(&dir);
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![CompletedTurn {
            content: String::new(),
            tool_calls: vec![
                tool_call_id("write_file", r#"{"path":"a.txt","content":"x"}"#, "c0"),
                tool_call_id("write_file", r#"{"path":"b.txt","content":"x"}"#, "c1"),
                tool_call_id("write_file", r#"{"path":"c.txt","content":"x"}"#, "c2"),
            ],
            thinking: None,
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
    let mut io = TestIo::new(Approval::Declined);

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
    // The user aborts at the first of two calls; both must still receive a tool_result, or the exchange
    // is invalid and unpersistable.
    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![CompletedTurn {
            content: String::new(),
            tool_calls: vec![
                tool_call_id("delete_file", r#"{"path":"a.txt"}"#, "c0"),
                tool_call_id("delete_file", r#"{"path":"b.txt"}"#, "c1"),
            ],
            thinking: None,
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
    let mut io = TestIo::new(Approval::Aborted);

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

#[tokio::test]
async fn provider_failure_propagates_after_finishing_the_render() {
    // Render cleanup must run on the failure path too. Moving `finish_round` after the `?` would pass
    // every other test but break this.
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
    let mut io = TestIo::new(Approval::Approved);

    let result = agent_loop
        .run(&mut conversation, &sandbox, ApprovalMode::Default, &mut io)
        .await;

    assert!(
        matches!(result, Err(AgentError::Provider(_))),
        "the provider error must propagate"
    );
    assert_eq!(
        io.finishes, 1,
        "finish_round must run exactly once before the error propagates"
    );
}

#[tokio::test]
async fn wall_clock_checkpoint_fires_with_an_elapsed_reason() {
    // A zero wall-clock budget trips the time leg; the cap is large, so the reason must be Elapsed.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();
    let sandbox = confined_sandbox(&dir);
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![CompletedTurn {
            content: "x".to_string(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
            thinking: None,
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
    let mut io = TestIo::new(Approval::Declined);

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

/// The production seam the other tests skip: a turn through the REAL `Bridge` adapter, asserting the
/// engine emits `Began` first. A regression here is exactly "first message does nothing, no spinner".
#[tokio::test]
async fn run_through_the_real_bridge_emits_began_first_then_content() {
    use crate::modules::tui::infrastructure::runtime::bridge::{Bridge, CancelToken, EngineMsg};
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
                thinking: None,
            })
        }
    }

    let dir = TempDir::new().unwrap();
    let sandbox = FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap();
    let agent_loop = AgentLoop::new(
        Arc::new(EmittingProvider),
        registry_for_tests(),
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

#[test]
fn tool_results_are_english() {
    // The tool-result consts are model-facing protocol text and must stay English; the user-facing pt-BR
    // prompts live in the Bridge adapter.
    let messages = [
        TOOL_RESULT_PLAN_PRESENTED,
        TOOL_RESULT_PLAN_BLOCKED,
        TOOL_RESULT_IGNORED_SESSION_ENDED,
        TOOL_RESULT_IGNORED_CHECKPOINT,
        TOOL_RESULT_IGNORED_USER_ABORT,
    ];
    let pt_br_markers = [
        "ignorada",
        "apresentado",
        "encerra",
        "sessão",
        "execução",
        "interrompida",
        "usuário",
        "aprovação",
    ];
    for message in messages {
        let lower = message.to_lowercase();
        for marker in pt_br_markers {
            assert!(
                !lower.contains(marker),
                "tool-result const carries pt-BR marker {marker:?}: {message:?}"
            );
        }
    }
    assert!(TOOL_RESULT_PLAN_PRESENTED.contains("presented"));
    assert!(TOOL_RESULT_IGNORED_USER_ABORT.starts_with("ignored:"));
}
