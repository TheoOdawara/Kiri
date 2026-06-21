use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::modules::agent::application::agent_io::AgentIo;
use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::agent::domain::conversation::Conversation;
use crate::modules::agent::domain::message::Message;
use crate::modules::provider::application::completion_provider::{CompletionProvider, TurnRequest};
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::application::tool::ToolOutcome;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::kernel::error::AgentError;

/// Whether a user turn ran to completion or the user ended the session at a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Aborted,
}

/// The agent loop. For one user turn: stream the assistant, then while it requests tools, confirm each
/// call through the UI, execute approved ones against the sandbox, feed the results back, and repeat
/// until the model stops requesting tools — guarded by a wall-clock checkpoint against runaways.
pub struct RunTurn {
    provider: Arc<dyn CompletionProvider>,
    registry: ToolRegistry,
    schemas: Vec<Value>,
    model: String,
    checkpoint_budget: Duration,
}

impl RunTurn {
    pub fn new(
        provider: Arc<dyn CompletionProvider>,
        registry: ToolRegistry,
        model: String,
        checkpoint_budget: Duration,
    ) -> Self {
        let schemas = registry.schemas();
        Self {
            provider,
            registry,
            schemas,
            model,
            checkpoint_budget,
        }
    }

    /// Drive one user turn to completion. The conversation must already hold the user message. On a
    /// provider failure the error is returned (the caller renders it and rolls back a dangling user
    /// message); `Aborted` means the user ended the session at a prompt.
    pub async fn run<IO: AgentIo>(
        &self,
        conversation: &mut Conversation,
        sandbox: &Sandbox,
        io: &mut IO,
    ) -> Result<TurnOutcome, AgentError> {
        let mut checkpoint = Instant::now();
        loop {
            io.begin_turn();
            let result = self
                .provider
                .complete(
                    TurnRequest {
                        messages: conversation.messages(),
                        model: &self.model,
                        tools: &self.schemas,
                    },
                    io,
                )
                .await;
            let _ = io.finish_turn();
            let turn = result?;

            if turn.tool_calls.is_empty() {
                // Plain text turn (also covers a degenerate tool-call finish with no parsed calls).
                conversation.push(Message::assistant_text(turn.content));
                return Ok(TurnOutcome::Completed);
            }

            let calls = turn.tool_calls;
            let narration = (!turn.content.is_empty()).then_some(turn.content);
            conversation.push(Message::assistant_tool_calls(narration, calls.clone()));

            for call in &calls {
                let outcome = match self.registry.confirm(sandbox, call) {
                    Some(confirmation) => match io.decide(&confirmation).await {
                        Approval::Approved => self.registry.execute(sandbox, call),
                        Approval::Declined => ToolOutcome::Declined,
                        Approval::Aborted => return Ok(TurnOutcome::Aborted),
                    },
                    None => self.registry.execute(sandbox, call),
                };
                conversation.push(Message::tool_result(
                    call.id.as_str(),
                    outcome.into_message_content(),
                ));
            }

            if checkpoint.elapsed() >= self.checkpoint_budget {
                let minutes = checkpoint.elapsed().as_secs() / 60;
                match io.confirm_continue(minutes).await {
                    Approval::Approved => checkpoint = Instant::now(),
                    Approval::Declined => return Ok(TurnOutcome::Completed),
                    Approval::Aborted => return Ok(TurnOutcome::Aborted),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::modules::agent::application::approval_policy::ApprovalPolicy;
    use crate::modules::agent::application::presenter::Presenter;
    use crate::modules::agent::domain::completed_turn::CompletedTurn;
    use crate::modules::agent::domain::role::Role;
    use crate::modules::agent::domain::stream_event::StreamEvent;
    use crate::modules::provider::application::completion_provider::EventSink;
    use crate::modules::tools::application::tool::Confirmation;
    use crate::modules::tools::infrastructure::fs::default_fs_tools;
    use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            let pid = std::process::id();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            path.push(format!("t-cli-runturn-{tag}-{pid}-{n}"));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
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
        fn notice(&mut self, _line: &str) {}
    }

    #[async_trait::async_trait(?Send)]
    impl ApprovalPolicy for ScriptedIo {
        async fn decide(&mut self, _confirmation: &Confirmation) -> Approval {
            self.0
        }
        async fn confirm_continue(&mut self, _minutes: u64) -> Approval {
            self.0
        }
    }

    fn tool_call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: "c1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    fn run_turn_with(turns: Vec<CompletedTurn>) -> RunTurn {
        let provider = Arc::new(ScriptedProvider {
            turns: Mutex::new(VecDeque::from(turns)),
        });
        RunTurn::new(
            provider,
            ToolRegistry::new(default_fs_tools()),
            "model".to_string(),
            Duration::from_secs(3600),
        )
    }

    #[tokio::test]
    async fn run_drives_a_tool_turn_then_a_text_turn() {
        let dir = TempDir::new("seq");
        std::fs::write(dir.path.join("a.txt"), b"hello").unwrap();
        let sandbox = Sandbox::new(&dir.path).unwrap();

        let run_turn = run_turn_with(vec![
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

        let outcome = run_turn
            .run(&mut conversation, &sandbox, &mut io)
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
        let dir = TempDir::new("abort");
        let sandbox = Sandbox::new(&dir.path).unwrap();
        let run_turn = run_turn_with(vec![CompletedTurn {
            content: String::new(),
            tool_calls: vec![tool_call("read_file", r#"{"path":"a.txt"}"#)],
        }]);

        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("read a.txt"));
        let mut io = ScriptedIo(Approval::Aborted);

        let outcome = run_turn
            .run(&mut conversation, &sandbox, &mut io)
            .await
            .unwrap();
        assert_eq!(outcome, TurnOutcome::Aborted);
    }
}
