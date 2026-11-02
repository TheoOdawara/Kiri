use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol_schema::v1::{
    CLIENT_METHOD_NAMES, ContentBlock, ContentChunk, Error, PromptResponse, RequestId, SessionId,
    SessionNotification, SessionUpdate, StopReason, ToolCall, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind,
};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use tokio::time::{Instant, sleep, sleep_until};

use crate::protocol::jsonrpc;
use crate::provider::openai_compatible::{
    OpenAiCompatible, ProviderError, ProviderEvent, ReplyEnd,
};
use crate::session::{Message, Role, Session};

const INTERNAL_ERROR: i32 = -32603;
const BATCH_INTERVAL: Duration = Duration::from_millis(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) struct Turn {
    pub(crate) session: Arc<Mutex<Session>>,
    pub(crate) session_id: SessionId,
    pub(crate) request_id: RequestId,
    pub(crate) prompt: String,
    pub(crate) cancelled: Arc<Notify>,
    pub(crate) outgoing: mpsc::Sender<Vec<u8>>,
}

enum Ending {
    Finished(ReplyEnd),
    Cancelled,
    Failed(ProviderError),
}

#[derive(Debug)]
struct ClientGone;

enum StreamFailure {
    Provider(ProviderError),
    ClientGone,
}

impl From<ProviderError> for StreamFailure {
    fn from(cause: ProviderError) -> Self {
        Self::Provider(cause)
    }
}

impl From<ClientGone> for StreamFailure {
    fn from(_: ClientGone) -> Self {
        Self::ClientGone
    }
}

pub(crate) async fn run_turn(turn: Turn) {
    let (provider, mut messages) = {
        let session = turn
            .session
            .lock()
            .expect("no turn panics holding a session");
        (Arc::clone(&session.provider), session.messages.clone())
    };
    messages.push(Message {
        role: Role::User,
        text: turn.prompt,
        reasoning: None,
    });

    let mut output = TurnOutput::new(turn.session_id, turn.outgoing.clone());
    let ending = tokio::select! {
        biased;
        () = turn.cancelled.notified() => Ending::Cancelled,
        reply = stream_reply(&provider, &messages, &mut output) => match reply {
            Ok(end) => Ending::Finished(end),
            Err(StreamFailure::Provider(cause)) => Ending::Failed(cause),
            Err(StreamFailure::ClientGone) => Ending::Cancelled,
        },
        () = send_heartbeats(&turn.outgoing, HEARTBEAT_INTERVAL) => Ending::Cancelled,
    };
    output.finish().await;

    let result = {
        let mut session = turn
            .session
            .lock()
            .expect("no turn panics holding a session");
        session.running_turn = None;
        match ending {
            Ending::Finished(end) => {
                if end != ReplyEnd::Refused && !output.text.trim().is_empty() {
                    messages.push(Message {
                        role: Role::Assistant,
                        text: output.text,
                        reasoning: (!output.reasoning.is_empty()).then_some(output.reasoning),
                    });
                    session.messages = messages;
                }
                stop_reason(match end {
                    ReplyEnd::Complete => StopReason::EndTurn,
                    ReplyEnd::OutputLimit => StopReason::MaxTokens,
                    ReplyEnd::Refused => StopReason::Refusal,
                })
            }
            Ending::Cancelled => stop_reason(StopReason::Cancelled),
            Ending::Failed(cause) => {
                let (failure, status) = cause.log_fields();
                tracing::warn!(failure, status, "a turn failed");
                Err(Error::new(INTERNAL_ERROR, cause.to_string()))
            }
        }
    };
    let _ = turn
        .outgoing
        .send(jsonrpc::response_line(turn.request_id, result))
        .await;
}

fn stop_reason(stop_reason: StopReason) -> Result<Value, Error> {
    serde_json::to_value(PromptResponse::new(stop_reason)).map_err(|_| Error::internal_error())
}

async fn send_heartbeats(outgoing: &mpsc::Sender<Vec<u8>>, interval: Duration) {
    loop {
        sleep(interval).await;
        let line = jsonrpc::notification_line(jsonrpc::ALIVE_METHOD, json!({}));
        if outgoing.send(line).await.is_err() {
            return;
        }
    }
}

async fn stream_reply(
    provider: &OpenAiCompatible,
    messages: &[Message],
    output: &mut TurnOutput,
) -> Result<ReplyEnd, StreamFailure> {
    let mut reply = provider.send(messages).await?;
    loop {
        let deadline = output.run.as_ref().map(|run| run.deadline);
        let event = tokio::select! {
            () = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                output.send_run().await?;
                continue;
            }
            event = reply.next_event() => event?,
        };
        let Some(event) = event else {
            return Ok(reply.end());
        };
        output.accept(event).await?;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum RunKind {
    Reasoning,
    Text,
}

struct OpenRun {
    kind: RunKind,
    text: String,
    deadline: Instant,
}

struct OpenToolCall {
    index: u64,
    id: String,
    arguments: String,
}

struct TurnOutput {
    session_id: SessionId,
    outgoing: mpsc::Sender<Vec<u8>>,
    run: Option<OpenRun>,
    tool_calls: Vec<OpenToolCall>,
    text: String,
    reasoning: String,
}

impl TurnOutput {
    fn new(session_id: SessionId, outgoing: mpsc::Sender<Vec<u8>>) -> Self {
        Self {
            session_id,
            outgoing,
            run: None,
            tool_calls: Vec::new(),
            text: String::new(),
            reasoning: String::new(),
        }
    }

    async fn accept(&mut self, event: ProviderEvent) -> Result<(), ClientGone> {
        match event {
            ProviderEvent::Reasoning(delta) => {
                self.extend_run(RunKind::Reasoning, &delta).await?;
                self.reasoning.push_str(&delta);
                Ok(())
            }
            ProviderEvent::Text(delta) => {
                self.extend_run(RunKind::Text, &delta).await?;
                self.text.push_str(&delta);
                Ok(())
            }
            ProviderEvent::ToolCallFragment {
                index,
                id,
                name,
                arguments,
            } => {
                if !self.tool_calls.iter().any(|call| call.index == index) {
                    self.open_tool_call(index, id, name).await?;
                }
                if let Some(arguments) = arguments
                    && let Some(call) = self.tool_calls.iter_mut().find(|call| call.index == index)
                {
                    call.arguments.push_str(&arguments);
                }
                Ok(())
            }
        }
    }

    async fn extend_run(&mut self, kind: RunKind, delta: &str) -> Result<(), ClientGone> {
        if self.run.as_ref().is_some_and(|run| run.kind != kind) {
            self.send_run().await?;
        }
        let run = self.run.get_or_insert_with(|| OpenRun {
            kind,
            text: String::new(),
            deadline: Instant::now() + BATCH_INTERVAL,
        });
        run.text.push_str(delta);
        Ok(())
    }

    async fn open_tool_call(
        &mut self,
        index: u64,
        id: Option<String>,
        name: Option<String>,
    ) -> Result<(), ClientGone> {
        self.send_run().await?;
        let id = id.unwrap_or_else(|| format!("call-{index}"));
        let tool_call = ToolCall::new(id.clone(), name.unwrap_or_default())
            .kind(ToolKind::Other)
            .status(ToolCallStatus::Pending);
        self.send_update(SessionUpdate::ToolCall(tool_call)).await?;
        self.tool_calls.push(OpenToolCall {
            index,
            id,
            arguments: String::new(),
        });
        Ok(())
    }

    async fn send_run(&mut self) -> Result<(), ClientGone> {
        if let Some(run) = &self.run {
            let chunk = ContentChunk::new(ContentBlock::from(run.text.clone()));
            let update = match run.kind {
                RunKind::Reasoning => SessionUpdate::AgentThoughtChunk(chunk),
                RunKind::Text => SessionUpdate::AgentMessageChunk(chunk),
            };
            self.send_update(update).await?;
        }
        self.run = None;
        Ok(())
    }

    async fn finish(&mut self) {
        let _ = self.send_run().await;
        for call in &self.tool_calls {
            let fields = ToolCallUpdateFields::new()
                .status(ToolCallStatus::Failed)
                .raw_input(Value::String(call.arguments.clone()));
            let update = ToolCallUpdate::new(call.id.clone(), fields);
            let _ = self
                .send_update(SessionUpdate::ToolCallUpdate(update))
                .await;
        }
    }

    async fn send_update(&self, update: SessionUpdate) -> Result<(), ClientGone> {
        let notification = SessionNotification::new(self.session_id.clone(), update);
        self.outgoing
            .send(jsonrpc::notification_line(
                CLIENT_METHOD_NAMES.session_update,
                notification,
            ))
            .await
            .map_err(|_| ClientGone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> (TurnOutput, mpsc::Receiver<Vec<u8>>) {
        let (outgoing, received) = mpsc::channel(16);
        (TurnOutput::new(SessionId::new("s"), outgoing), received)
    }

    fn next_update(received: &mut mpsc::Receiver<Vec<u8>>) -> Value {
        let line = received.try_recv().unwrap();
        serde_json::from_slice::<Value>(&line).unwrap()["params"]["update"].clone()
    }

    #[tokio::test]
    async fn heartbeats_repeat_and_stop_when_the_client_is_gone() {
        let (outgoing, mut received) = mpsc::channel(4);
        let beats = tokio::spawn(async move {
            send_heartbeats(&outgoing, Duration::from_millis(1)).await;
        });
        for _ in 0..2 {
            let line = received.recv().await.unwrap();
            let message: Value = serde_json::from_slice(&line).unwrap();
            assert_eq!(message["method"], "_kiri/alive");
            assert!(message.get("id").is_none());
        }
        drop(received);
        beats.await.unwrap();
    }

    #[tokio::test]
    async fn a_change_of_kind_sends_the_open_run_and_finish_sends_the_last() {
        let (mut output, mut received) = output();
        for event in [
            ProviderEvent::Reasoning("a".to_owned()),
            ProviderEvent::Reasoning("b".to_owned()),
            ProviderEvent::Text("c".to_owned()),
        ] {
            output.accept(event).await.unwrap();
        }
        output.finish().await;

        let thought = next_update(&mut received);
        assert_eq!(thought["sessionUpdate"], "agent_thought_chunk");
        assert_eq!(thought["content"]["text"], "ab");
        let message = next_update(&mut received);
        assert_eq!(message["sessionUpdate"], "agent_message_chunk");
        assert_eq!(message["content"]["text"], "c");
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_tool_call_flushes_the_run_goes_out_at_once_and_fails_at_the_finish() {
        let (mut output, mut received) = output();
        for event in [
            ProviderEvent::Text("t".to_owned()),
            ProviderEvent::ToolCallFragment {
                index: 3,
                id: None,
                name: Some("lookup".to_owned()),
                arguments: Some("{\"a\"".to_owned()),
            },
            ProviderEvent::ToolCallFragment {
                index: 3,
                id: None,
                name: None,
                arguments: Some(":1}".to_owned()),
            },
        ] {
            output.accept(event).await.unwrap();
        }

        assert_eq!(next_update(&mut received)["content"]["text"], "t");
        let started = next_update(&mut received);
        assert_eq!(started["sessionUpdate"], "tool_call");
        assert_eq!(started["toolCallId"], "call-3");
        assert_eq!(started["title"], "lookup");
        let tool_call: ToolCall = serde_json::from_value(started).unwrap();
        assert_eq!(tool_call.kind, ToolKind::Other);
        assert_eq!(tool_call.status, ToolCallStatus::Pending);
        assert!(received.try_recv().is_err());

        output.finish().await;
        let finished = next_update(&mut received);
        assert_eq!(finished["sessionUpdate"], "tool_call_update");
        assert_eq!(finished["toolCallId"], "call-3");
        assert_eq!(finished["status"], "failed");
        assert_eq!(finished["rawInput"], "{\"a\":1}");
    }
}
