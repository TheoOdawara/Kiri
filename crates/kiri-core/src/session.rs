use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_client_protocol_schema::v1::{
    AGENT_METHOD_NAMES, CancelNotification, ContentBlock, Error, NewSessionRequest,
    NewSessionResponse, PromptRequest, Request, RequestId, SessionId,
};
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinSet;

use crate::agent;
use crate::env::ConfigError;
use crate::protocol::jsonrpc::{self, Incoming};
use crate::provider::openai_compatible::OpenAiCompatible;

const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
const SESSION_NOT_FOUND: i32 = -32002;

#[derive(Clone, Copy)]
pub(crate) enum Role {
    User,
    Assistant,
}

impl Role {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Message {
    pub(crate) role: Role,
    pub(crate) text: String,
}

pub(crate) struct Session {
    pub(crate) provider: Arc<OpenAiCompatible>,
    pub(crate) messages: Vec<Message>,
    pub(crate) running_turn: Option<Arc<Notify>>,
}

pub(crate) struct Host {
    provider: Result<Arc<OpenAiCompatible>, ConfigError>,
    stop_listening: mpsc::Sender<oneshot::Sender<()>>,
}

impl Host {
    pub(crate) fn new(
        provider: Result<OpenAiCompatible, ConfigError>,
        stop_listening: mpsc::Sender<oneshot::Sender<()>>,
    ) -> Self {
        Self {
            provider: provider.map(Arc::new),
            stop_listening,
        }
    }

    async fn stop_listening(&self) {
        let (stopped, confirmation) = oneshot::channel();
        if self.stop_listening.send(stopped).await.is_ok() {
            let _ = confirmation.await;
        }
    }
}

pub(crate) struct ConnectionClosed;

pub(crate) struct Sessions {
    host: Arc<Host>,
    outgoing: mpsc::Sender<Vec<u8>>,
    sessions: HashMap<SessionId, Arc<Mutex<Session>>>,
    turns: JoinSet<()>,
}

impl Sessions {
    pub(crate) fn new(host: Arc<Host>, outgoing: mpsc::Sender<Vec<u8>>) -> Self {
        Self {
            host,
            outgoing,
            sessions: HashMap::new(),
            turns: JoinSet::new(),
        }
    }

    pub(crate) async fn handle(&mut self, line: &[u8]) -> Result<(), ConnectionClosed> {
        while self.turns.try_join_next().is_some() {}
        match jsonrpc::decode(line) {
            Incoming::Malformed(error) => {
                self.answer(RequestId::Null, Err(error)).await?;
            }
            Incoming::Notification { method, params } => {
                if method == AGENT_METHOD_NAMES.session_cancel {
                    self.cancel(params);
                }
            }
            Incoming::Request(request) => self.handle_request(request).await?,
        }
        Ok(())
    }

    async fn handle_request(&mut self, request: Request<Value>) -> Result<(), ConnectionClosed> {
        let Request { id, method, params } = request;
        let params = params.unwrap_or_default();
        if &*method == AGENT_METHOD_NAMES.session_prompt {
            return match self.start_turn(id.clone(), params) {
                Ok(()) => Ok(()),
                Err(error) => self.answer(id, Err(error)).await,
            };
        }
        let result = if &*method == AGENT_METHOD_NAMES.initialize {
            jsonrpc::initialize(params)
        } else if &*method == AGENT_METHOD_NAMES.session_new {
            self.new_session(params).await
        } else {
            Err(Error::method_not_found())
        };
        self.answer(id, result).await
    }

    async fn answer(
        &self,
        id: RequestId,
        result: Result<Value, Error>,
    ) -> Result<(), ConnectionClosed> {
        self.outgoing
            .send(jsonrpc::response_line(id, result))
            .await
            .map_err(|_| ConnectionClosed)
    }

    async fn new_session(&mut self, params: Value) -> Result<Value, Error> {
        let provider = match &self.host.provider {
            Ok(provider) => Arc::clone(provider),
            Err(cause) => {
                let error = Error::new(INTERNAL_ERROR, cause.to_string());
                self.host.stop_listening().await;
                return Err(error);
            }
        };
        let request: NewSessionRequest =
            serde_json::from_value(params).map_err(|_| Error::invalid_params())?;
        if !request.cwd.is_absolute() {
            return Err(Error::new(INVALID_PARAMS, "cwd must be an absolute path."));
        }
        let session_id = SessionId::new(new_session_id());
        self.sessions.insert(
            session_id.clone(),
            Arc::new(Mutex::new(Session {
                provider,
                messages: Vec::new(),
                running_turn: None,
            })),
        );
        serde_json::to_value(NewSessionResponse::new(session_id))
            .map_err(|_| Error::internal_error())
    }

    fn start_turn(&mut self, id: RequestId, params: Value) -> Result<(), Error> {
        if holds_a_refused_block(&params) {
            return Err(Error::new(
                INVALID_PARAMS,
                "Kiri accepts only text and resource links in a prompt.",
            ));
        }
        let request: PromptRequest =
            serde_json::from_value(params).map_err(|_| Error::invalid_params())?;
        let session = self.sessions.get(&request.session_id).ok_or_else(|| {
            Error::new(
                SESSION_NOT_FOUND,
                format!("Unknown session: {}.", request.session_id),
            )
        })?;
        let prompt = prompt_text(&request.prompt);
        if prompt.trim().is_empty() {
            return Err(Error::new(INVALID_PARAMS, "The prompt is empty."));
        }
        let cancelled = Arc::new(Notify::new());
        {
            let mut session = session.lock().expect("no turn panics holding a session");
            if session.running_turn.is_some() {
                return Err(Error::new(
                    INVALID_PARAMS,
                    "A turn is already running in this session.",
                ));
            }
            session.running_turn = Some(Arc::clone(&cancelled));
        }
        self.turns.spawn(agent::run_turn(agent::Turn {
            session: Arc::clone(session),
            session_id: request.session_id,
            request_id: id,
            prompt,
            cancelled,
            outgoing: self.outgoing.clone(),
        }));
        Ok(())
    }

    fn cancel(&self, params: Option<Value>) {
        let Some(request) =
            params.and_then(|params| serde_json::from_value::<CancelNotification>(params).ok())
        else {
            return;
        };
        if let Some(session) = self.sessions.get(&request.session_id)
            && let Some(cancelled) = &session
                .lock()
                .expect("no turn panics holding a session")
                .running_turn
        {
            cancelled.notify_one();
        }
    }
}

fn holds_a_refused_block(params: &Value) -> bool {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                !matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("text" | "resource_link")
                )
            })
        })
}

fn prompt_text(blocks: &[ContentBlock]) -> String {
    let parts: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            ContentBlock::ResourceLink(link) => Some(link.uri.as_str()),
            _ => None,
        })
        .collect();
    parts.join("\n")
}

// shortcut: the random bits come from std's OS-seeded hasher and are not cryptographic, upgrade when std::random is stable or an ID becomes a secret.
fn new_session_id() -> String {
    let random = || RandomState::new().build_hasher().finish();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    uuid_v7(millis, random(), random())
}

fn uuid_v7(millis: u64, random_high: u64, random_low: u64) -> String {
    let variant_and_random = (random_low & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-7{:03x}-{:04x}-{:012x}",
        (millis >> 16) & 0xffff_ffff,
        millis & 0xffff,
        random_high & 0xfff,
        variant_and_random >> 48,
        variant_and_random & 0xffff_ffff_ffff,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_id_is_a_uuid_v7_with_the_time_in_front() {
        assert_eq!(
            uuid_v7(0x0190_1234_5678, u64::MAX, 0),
            "01901234-5678-7fff-8000-000000000000"
        );
        assert_eq!(
            uuid_v7(u64::MAX, 0, u64::MAX),
            "ffffffff-ffff-7000-bfff-ffffffffffff"
        );
    }

    #[test]
    fn two_session_ids_differ() {
        assert_ne!(new_session_id(), new_session_id());
    }
}
