mod endpoint;
pub(crate) mod framing;
pub(crate) mod jsonrpc;
pub(crate) mod transport;

use std::io;
use std::path::Path;
use std::time::Duration;

use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::rpc::{JsonRpcMessage, Notification};
use agent_client_protocol_schema::v1::{
    AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES, Implementation, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, Request, RequestId,
    Response, SessionNotification,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};

pub use agent_client_protocol_schema::v1::{
    ContentBlock, ContentChunk, SessionId, SessionUpdate, StopReason, ToolCallId,
};
pub use endpoint::{Endpoint, EndpointError};

const TURN_SILENCE: Duration = Duration::from_secs(70);

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("The Kiri daemon pipe belongs to another user. Refusing to connect.")]
    ForeignPipe,
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub enum TurnEvent {
    Update(Box<SessionUpdate>),
    Ended(StopReason),
    Failed(String),
}

pub struct Connection {
    reader: BufReader<ReadHalf<transport::ClientStream>>,
    writer: WriteHalf<transport::ClientStream>,
    next_request_id: i64,
}

pub async fn connect(endpoint: &Endpoint) -> Result<Connection, ConnectError> {
    let stream = transport::connect(endpoint).await?;
    let (reader, writer) = tokio::io::split(stream);
    let mut connection = Connection {
        reader: BufReader::new(reader),
        writer,
        next_request_id: 1,
    };
    connection.initialize().await?;
    Ok(connection)
}

impl Connection {
    async fn initialize(&mut self) -> io::Result<()> {
        let params = InitializeRequest::new(ProtocolVersion::V1)
            .client_info(Implementation::new("kiri", env!("CARGO_PKG_VERSION")));
        let _: InitializeResponse = self.call(AGENT_METHOD_NAMES.initialize, params).await?;
        Ok(())
    }

    pub async fn new_session(&mut self, cwd: &Path) -> io::Result<SessionId> {
        let response: NewSessionResponse = self
            .call(AGENT_METHOD_NAMES.session_new, NewSessionRequest::new(cwd))
            .await?;
        Ok(response.session_id)
    }

    pub async fn send_prompt(&mut self, session: &SessionId, text: &str) -> io::Result<()> {
        let prompt = PromptRequest::new(session.clone(), vec![ContentBlock::from(text)]);
        self.send(AGENT_METHOD_NAMES.session_prompt, prompt).await
    }

    pub async fn next_turn_event(&mut self) -> io::Result<TurnEvent> {
        loop {
            let line = read_line_within(&mut self.reader, TURN_SILENCE).await?;
            if let Some(event) = turn_event_of(&line)? {
                return Ok(event);
            }
        }
    }

    pub async fn closed(&mut self) {
        while let Ok(Some(_)) = framing::read_line(&mut self.reader).await {}
    }

    async fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
    ) -> io::Result<R> {
        self.send(method, params).await?;
        let line = self.read_line().await?;
        let answer: JsonRpcMessage<Response<R>> = serde_json::from_slice(&line)?;
        match answer.into_inner() {
            Response::Result { result, .. } => Ok(result),
            Response::Error { error, .. } => Err(io::Error::other(error.message)),
        }
    }

    async fn send<P: Serialize>(&mut self, method: &str, params: P) -> io::Result<()> {
        let request = JsonRpcMessage::wrap(Request {
            id: RequestId::Number(self.next_request_id),
            method: method.into(),
            params: Some(params),
        });
        self.next_request_id += 1;
        self.writer.write_all(&jsonrpc::line_of(&request)).await
    }

    async fn read_line(&mut self) -> io::Result<Vec<u8>> {
        framing::read_line(&mut self.reader)
            .await?
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}

async fn read_line_within<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    silence: Duration,
) -> io::Result<Vec<u8>> {
    tokio::time::timeout(silence, framing::read_line(reader))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??
        .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
}

fn turn_event_of(line: &[u8]) -> io::Result<Option<TurnEvent>> {
    let message: Value = serde_json::from_slice(line)?;
    if let Some(method) = message.get("method") {
        if method != CLIENT_METHOD_NAMES.session_update {
            return Ok(None);
        }
        let notification: JsonRpcMessage<Notification<SessionNotification>> =
            serde_json::from_value(message)?;
        return Ok(notification
            .into_inner()
            .params
            .map(|params| TurnEvent::Update(Box::new(params.update))));
    }
    let response: JsonRpcMessage<Response<PromptResponse>> = serde_json::from_value(message)?;
    Ok(Some(match response.into_inner() {
        Response::Result { result, .. } => TurnEvent::Ended(result.stop_reason),
        Response::Error { error, .. } => TurnEvent::Failed(error.message),
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncWriteExt, BufReader, duplex};

    use super::*;

    #[test]
    fn the_alive_notification_is_no_turn_event() {
        let line = jsonrpc::notification_line(jsonrpc::ALIVE_METHOD, json!({}));
        assert!(turn_event_of(&line).unwrap().is_none());
    }

    #[tokio::test]
    async fn a_line_rearms_the_silence_and_silence_ends_the_wait() {
        let silence = Duration::from_millis(300);
        let (mut daemon, client) = duplex(64);
        let mut reader = BufReader::new(client);

        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            daemon.write_all(b"{}\n").await.unwrap();
            assert_eq!(read_line_within(&mut reader, silence).await.unwrap(), b"{}");
        }

        let failure = read_line_within(&mut reader, silence).await.unwrap_err();
        assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
    }
}
