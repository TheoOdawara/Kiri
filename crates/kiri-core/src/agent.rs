use std::sync::{Arc, Mutex};

use agent_client_protocol_schema::v1::{
    CLIENT_METHOD_NAMES, ContentBlock, ContentChunk, Error, PromptResponse, RequestId, SessionId,
    SessionNotification, SessionUpdate, StopReason,
};
use serde_json::Value;
use tokio::sync::{Notify, mpsc};

use crate::protocol::jsonrpc;
use crate::provider::openai_compatible::{OpenAiCompatible, ProviderError, ProviderEvent};
use crate::session::{Message, Role, Session};

const INTERNAL_ERROR: i32 = -32603;

pub(crate) struct Turn {
    pub(crate) session: Arc<Mutex<Session>>,
    pub(crate) session_id: SessionId,
    pub(crate) request_id: RequestId,
    pub(crate) prompt: String,
    pub(crate) cancelled: Arc<Notify>,
    pub(crate) outgoing: mpsc::Sender<Vec<u8>>,
}

enum Ending {
    Finished(String),
    Cancelled,
    Failed(ProviderError),
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
    });

    let ending = tokio::select! {
        biased;
        () = turn.cancelled.notified() => Ending::Cancelled,
        reply = stream_reply(&provider, &messages, &turn.session_id, &turn.outgoing) => match reply {
            Ok(reply) => Ending::Finished(reply),
            Err(cause) => Ending::Failed(cause),
        },
    };

    let result = {
        let mut session = turn
            .session
            .lock()
            .expect("no turn panics holding a session");
        session.running_turn = None;
        match ending {
            Ending::Finished(reply) => {
                messages.push(Message {
                    role: Role::Assistant,
                    text: reply,
                });
                session.messages = messages;
                stop_reason(StopReason::EndTurn)
            }
            Ending::Cancelled => stop_reason(StopReason::Cancelled),
            Ending::Failed(cause) => Err(Error::new(INTERNAL_ERROR, cause.to_string())),
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

// shortcut: one event per delta, #17 groups them into batches of about 10 ms.
async fn stream_reply(
    provider: &OpenAiCompatible,
    messages: &[Message],
    session_id: &SessionId,
    outgoing: &mpsc::Sender<Vec<u8>>,
) -> Result<String, ProviderError> {
    let mut reply = provider.send(messages).await?;
    let mut text = String::new();
    while let Some(ProviderEvent::Text(delta)) = reply.next_event().await? {
        text.push_str(&delta);
        let update = SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(delta))),
        );
        let line = jsonrpc::notification_line(CLIENT_METHOD_NAMES.session_update, update);
        if outgoing.send(line).await.is_err() {
            return Err(ProviderError);
        }
    }
    Ok(text)
}
