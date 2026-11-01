use std::collections::VecDeque;
use std::time::Duration;

use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, ClientBuilder, Response, StatusCode, Url, redirect};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::env::{Config, ConfigError};
use crate::session::Message;

const DONE: &str = "[DONE]";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const ENDPOINT_TEXT_CHARS: usize = 500;
const ERROR_BODY_BYTES: usize = 64 * 1024;
const KEY_MASK: &str = "***";

#[derive(Debug, Error, PartialEq)]
pub(crate) enum ProviderError {
    #[error("The endpoint rejected the API key (HTTP {0}).")]
    KeyRejected(u16),
    #[error("The endpoint is limiting requests (HTTP 429): {0}")]
    Limiting(String),
    #[error("The endpoint rejected the request (HTTP {status}): {text}")]
    Rejected { status: u16, text: String },
    #[error("The endpoint failed (HTTP {status}): {text}")]
    Failed { status: u16, text: String },
    #[error("Could not reach {base_url}: {cause}")]
    Unreachable { base_url: String, cause: String },
    #[error("The endpoint sent nothing for {0} seconds.")]
    Silent(u64),
    #[error("The endpoint closed the stream before the turn finished.")]
    StreamClosed,
    #[error("The endpoint failed during the stream: {0}")]
    StreamFailed(String),
    #[error("The endpoint sent a chunk Kiri cannot read: {0}")]
    UnreadableChunk(String),
}

impl ProviderError {
    pub(crate) fn log_fields(&self) -> (&'static str, Option<u16>) {
        match self {
            Self::KeyRejected(status) => ("key rejected", Some(*status)),
            Self::Limiting(_) => ("limiting requests", Some(429)),
            Self::Rejected { status, .. } => ("request rejected", Some(*status)),
            Self::Failed { status, .. } => ("endpoint failed", Some(*status)),
            Self::Unreachable { .. } => ("unreachable", None),
            Self::Silent(_) => ("silent", None),
            Self::StreamClosed => ("stream closed", None),
            Self::StreamFailed(_) => ("stream failed", None),
            Self::UnreadableChunk(_) => ("unreadable chunk", None),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ReplyEnd {
    Complete,
    OutputLimit,
    Refused,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ProviderEvent {
    Reasoning(String),
    Text(String),
    ToolCallFragment {
        index: u64,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
}

pub(crate) struct OpenAiCompatible {
    client: Client,
    url: String,
    display_url: String,
    api_key: String,
    connect_timeout: Duration,
    idle_timeout: Duration,
    model: String,
    request_params: Map<String, Value>,
    replays_reasoning: bool,
}

impl OpenAiCompatible {
    pub(crate) fn new(config: Config) -> Result<Self, ConfigError> {
        Self::with_timeouts(config, CONNECT_TIMEOUT, IDLE_TIMEOUT)
    }

    fn with_timeouts(
        config: Config,
        connect_timeout: Duration,
        idle_timeout: Duration,
    ) -> Result<Self, ConfigError> {
        let client = client_builder(connect_timeout, idle_timeout)
            .build()
            .map_err(|cause| ConfigError::HttpClient(cause.without_url().to_string()))?;
        let base_url = config.base_url;
        Ok(Self {
            client,
            url: format!(
                "{}/chat/completions",
                base_url.strip_suffix('/').unwrap_or(&base_url)
            ),
            display_url: without_credentials(&base_url).replace(&config.api_key, KEY_MASK),
            api_key: config.api_key,
            connect_timeout,
            idle_timeout,
            model: config.model,
            request_params: config.request_params,
            replays_reasoning: config.replays_reasoning,
        })
    }

    fn request_body(&self, messages: &[Message]) -> Value {
        let messages: Vec<Value> = messages
            .iter()
            .map(|message| {
                let mut entry = json!({"role": message.role.as_str(), "content": message.text});
                if self.replays_reasoning
                    && let Some(reasoning) = &message.reasoning
                {
                    entry["reasoning_content"] = json!(reasoning);
                }
                entry
            })
            .collect();
        let mut body = self.request_params.clone();
        body.insert("model".to_owned(), json!(self.model));
        body.insert("messages".to_owned(), json!(messages));
        body.insert("stream".to_owned(), json!(true));
        Value::Object(body)
    }

    pub(crate) async fn send(&self, messages: &[Message]) -> Result<Reply, ProviderError> {
        let body = self.request_body(messages);
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header(ACCEPT, "text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|cause| self.send_failure(&cause))?;
        let status = response.status();
        if !status.is_success() {
            let text = clean(&body_text(&read_error_body(response).await), &self.api_key);
            return Err(refusal(status, text));
        }
        Ok(Reply {
            events: response.bytes_stream().eventsource().boxed(),
            pending: VecDeque::new(),
            end: None,
            api_key: self.api_key.clone(),
            idle_seconds: self.idle_timeout.as_secs(),
        })
    }

    fn send_failure(&self, cause: &reqwest::Error) -> ProviderError {
        if cause.is_timeout() {
            let limit = if cause.is_connect() {
                self.connect_timeout
            } else {
                self.idle_timeout
            };
            return ProviderError::Silent(limit.as_secs());
        }
        ProviderError::Unreachable {
            base_url: self.display_url.clone(),
            cause: clean(&deepest_cause(cause), &self.api_key),
        }
    }
}

fn client_builder(connect_timeout: Duration, idle_timeout: Duration) -> ClientBuilder {
    Client::builder()
        .redirect(redirect::Policy::none())
        .connect_timeout(connect_timeout)
        .read_timeout(idle_timeout)
        .user_agent(concat!("kiri/", env!("CARGO_PKG_VERSION")))
}

fn without_credentials(base_url: &str) -> String {
    match Url::parse(base_url) {
        Ok(mut url) if !url.username().is_empty() || url.password().is_some() => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        _ => base_url.to_owned(),
    }
}

fn deepest_cause(error: &reqwest::Error) -> String {
    let mut deepest: &dyn std::error::Error = error;
    while let Some(cause) = deepest.source() {
        deepest = cause;
    }
    deepest.to_string()
}

fn refusal(status: StatusCode, text: String) -> ProviderError {
    let status = status.as_u16();
    match status {
        401 | 403 => ProviderError::KeyRejected(status),
        429 => ProviderError::Limiting(text),
        500..=599 => ProviderError::Failed { status, text },
        _ => ProviderError::Rejected { status, text },
    }
}

async fn read_error_body(mut response: Response) -> String {
    let mut body = Vec::new();
    while body.len() < ERROR_BODY_BYTES
        && let Ok(Some(chunk)) = response.chunk().await
    {
        body.extend_from_slice(&chunk);
    }
    String::from_utf8_lossy(&body).into_owned()
}

fn body_text(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|parsed| parsed.get("error").map(error_text))
        .unwrap_or_else(|| body.to_owned())
}

fn error_text(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map_or_else(|| error.to_string(), str::to_owned)
}

fn clean(text: &str, api_key: &str) -> String {
    text.replace(api_key, KEY_MASK)
        .trim()
        .chars()
        .take(ENDPOINT_TEXT_CHARS)
        .collect()
}

pub(crate) struct Reply {
    events: BoxStream<'static, Result<Event, EventStreamError<reqwest::Error>>>,
    pending: VecDeque<ProviderEvent>,
    end: Option<ReplyEnd>,
    api_key: String,
    idle_seconds: u64,
}

impl Reply {
    pub(crate) async fn next_event(&mut self) -> Result<Option<ProviderEvent>, ProviderError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            if self.end.is_some() {
                return Ok(None);
            }
            let event = match self.events.next().await {
                Some(Ok(event)) => event,
                Some(Err(EventStreamError::Transport(cause))) if cause.is_timeout() => {
                    return Err(ProviderError::Silent(self.idle_seconds));
                }
                Some(Err(EventStreamError::Transport(_))) | None => {
                    return Err(ProviderError::StreamClosed);
                }
                Some(Err(cause)) => {
                    return Err(ProviderError::UnreadableChunk(clean(
                        &cause.to_string(),
                        &self.api_key,
                    )));
                }
            };
            let (events, end) = read_chunk(&event.data, &self.api_key)?;
            self.end = end;
            self.pending.extend(events);
        }
    }

    pub(crate) fn end(&self) -> ReplyEnd {
        self.end.unwrap_or(ReplyEnd::Complete)
    }
}

fn read_chunk(
    data: &str,
    api_key: &str,
) -> Result<(Vec<ProviderEvent>, Option<ReplyEnd>), ProviderError> {
    if data == DONE {
        return Ok((Vec::new(), Some(ReplyEnd::Complete)));
    }
    let chunk: Value = serde_json::from_str(data)
        .map_err(|cause| ProviderError::UnreadableChunk(clean(&cause.to_string(), api_key)))?;
    if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
        return Err(ProviderError::StreamFailed(clean(
            &error_text(error),
            api_key,
        )));
    }
    let Some(choice) = chunk.pointer("/choices/0") else {
        return Ok((Vec::new(), None));
    };
    let delta = &choice["delta"];
    let mut events = Vec::new();
    if let Some(reasoning) =
        non_empty_text(&delta["reasoning_content"]).or_else(|| non_empty_text(&delta["reasoning"]))
    {
        events.push(ProviderEvent::Reasoning(reasoning));
    }
    if let Some(text) = non_empty_text(&delta["content"]) {
        events.push(ProviderEvent::Text(text));
    }
    let tool_calls = delta["tool_calls"].as_array().into_iter().flatten();
    for (position, tool_call) in tool_calls.enumerate() {
        events.push(ProviderEvent::ToolCallFragment {
            index: tool_call["index"].as_u64().unwrap_or(position as u64),
            id: non_empty_text(&tool_call["id"]),
            name: non_empty_text(&tool_call["function"]["name"]),
            arguments: non_empty_text(&tool_call["function"]["arguments"]),
        });
    }
    let end = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(|reason| match reason {
            "length" => ReplyEnd::OutputLimit,
            "content_filter" => ReplyEnd::Refused,
            _ => ReplyEnd::Complete,
        });
    Ok((events, end))
}

fn non_empty_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reqwest::dns::{Name, Resolve, Resolving};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn text(text: &str) -> ProviderEvent {
        ProviderEvent::Text(text.to_owned())
    }

    fn reasoning(text: &str) -> ProviderEvent {
        ProviderEvent::Reasoning(text.to_owned())
    }

    #[test]
    fn a_chunk_gives_its_events_and_how_the_stream_ended() {
        let cases = [
            (
                r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
                vec![text("hi")],
                None,
            ),
            (
                r#"{"choices":[{"delta":{"content":"end"},"finish_reason":"stop"}]}"#,
                vec![text("end")],
                Some(ReplyEnd::Complete),
            ),
            (
                r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
                vec![],
                Some(ReplyEnd::OutputLimit),
            ),
            (
                r#"{"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#,
                vec![],
                Some(ReplyEnd::Refused),
            ),
            (
                r#"{"choices":[{"delta":{},"finish_reason":"something_new"}]}"#,
                vec![],
                Some(ReplyEnd::Complete),
            ),
            (r#"{"choices":[],"usage":{}}"#, vec![], None),
            ("[DONE]", vec![], Some(ReplyEnd::Complete)),
            (
                r#"{"choices":[{"delta":{"reasoning_content":"a"}}]}"#,
                vec![reasoning("a")],
                None,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning":"b"}}]}"#,
                vec![reasoning("b")],
                None,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning_content":null,"reasoning":"c"}}]}"#,
                vec![reasoning("c")],
                None,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning_content":"","reasoning":"d"}}]}"#,
                vec![reasoning("d")],
                None,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning_content":"e","reasoning":"f"}}]}"#,
                vec![reasoning("e")],
                None,
            ),
        ];
        for (data, events, end) in cases {
            let (read_events, read_end) = read_chunk(data, "key").unwrap();
            assert_eq!(read_events, events, "{data}");
            assert_eq!(read_end, end, "{data}");
        }
    }

    #[test]
    fn a_chunk_with_every_part_gives_reasoning_then_text_then_tool_calls() {
        let data = r#"{"choices":[{"delta":{
            "tool_calls":[
                {"id":"x","function":{"name":"first","arguments":"{"}},
                {"index":7,"function":{"arguments":"}"}}
            ],
            "content":"t",
            "reasoning":"r"
        },"finish_reason":"tool_calls"}]}"#;
        let (events, end) = read_chunk(data, "key").unwrap();
        assert_eq!(
            events,
            vec![
                reasoning("r"),
                text("t"),
                ProviderEvent::ToolCallFragment {
                    index: 0,
                    id: Some("x".to_owned()),
                    name: Some("first".to_owned()),
                    arguments: Some("{".to_owned()),
                },
                ProviderEvent::ToolCallFragment {
                    index: 7,
                    id: None,
                    name: None,
                    arguments: Some("}".to_owned()),
                },
            ]
        );
        assert_eq!(end, Some(ReplyEnd::Complete));
    }

    #[test]
    fn a_chunk_that_is_not_json_is_an_error() {
        assert!(matches!(
            read_chunk("{oops", "key"),
            Err(ProviderError::UnreadableChunk(_))
        ));
    }

    #[test]
    fn an_error_in_a_chunk_is_masked_and_wins_over_its_choices() {
        let data = r#"{"choices":[{"delta":{"content":"x"}}],"error":{"message":"bad key"}}"#;
        assert_eq!(
            read_chunk(data, "key"),
            Err(ProviderError::StreamFailed("bad ***".to_owned()))
        );
        assert!(read_chunk(r#"{"choices":[],"error":null}"#, "key").is_ok());
    }

    #[test]
    fn endpoint_text_prefers_the_error_message_and_falls_back_to_the_body() {
        let cases = [
            (r#"{"error":{"message":"nope","code":1}}"#, "nope"),
            (r#"{"error":"nope"}"#, "nope"),
            (r#"{"error":{"code":1}}"#, r#"{"code":1}"#),
            (r#"{"detail":"nope"}"#, r#"{"detail":"nope"}"#),
            ("<html>", "<html>"),
        ];
        for (body, text) in cases {
            assert_eq!(body_text(body), text);
        }
    }

    #[test]
    fn cleaning_masks_the_key_before_it_cuts_by_characters() {
        let at_the_cut = format!("{}secret-key", "é".repeat(495));
        let cleaned = clean(&at_the_cut, "secret-key");
        assert_eq!(cleaned, format!("{}***", "é".repeat(495)));

        let long = "é".repeat(600);
        assert_eq!(clean(&long, "secret-key").chars().count(), 500);
    }

    #[test]
    fn the_displayed_base_url_has_no_credentials_and_no_key() {
        assert_eq!(
            without_credentials("https://user:pass@example.com/v1"),
            "https://example.com/v1"
        );
        assert_eq!(without_credentials("http://a"), "http://a");

        let provider = provider_on("http://a/v1?token=the-key", SHORT, SHORT);
        assert_eq!(provider.display_url, "http://a/v1?token=***");
    }

    const SHORT: Duration = Duration::from_secs(1);

    fn provider_on(base_url: &str, connect: Duration, idle: Duration) -> OpenAiCompatible {
        OpenAiCompatible::with_timeouts(
            Config {
                base_url: base_url.to_owned(),
                api_key: "the-key".to_owned(),
                model: "model".to_owned(),
                request_params: Map::new(),
                replays_reasoning: false,
            },
            connect,
            idle,
        )
        .unwrap_or_else(|_| panic!("no client"))
    }

    fn prompt() -> Vec<Message> {
        vec![Message {
            role: crate::session::Role::User,
            text: "hello".to_owned(),
            reasoning: None,
        }]
    }

    #[tokio::test]
    async fn an_endpoint_silent_before_its_headers_fails_at_the_idle_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let _accepted = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });

        let failure = provider_on(&base_url, SHORT, SHORT)
            .send(&prompt())
            .await
            .err();

        assert_eq!(failure, Some(ProviderError::Silent(1)));
    }

    #[tokio::test]
    async fn an_endpoint_silent_after_its_headers_fails_at_the_idle_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let _accepted = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                .await
                .unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });

        let mut reply = provider_on(&base_url, SHORT, SHORT)
            .send(&prompt())
            .await
            .unwrap_or_else(|_| panic!("no reply"));

        assert_eq!(reply.next_event().await, Err(ProviderError::Silent(1)));
    }

    struct NeverResolves;

    impl Resolve for NeverResolves {
        fn resolve(&self, _: Name) -> Resolving {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn a_connection_never_made_fails_at_the_connect_timeout() {
        let idle = Duration::from_secs(30);
        let mut provider = provider_on("http://kiri.invalid", SHORT, idle);
        provider.client = client_builder(SHORT, idle)
            .dns_resolver(Arc::new(NeverResolves))
            .build()
            .unwrap();

        let failure = provider.send(&prompt()).await.err();

        assert_eq!(failure, Some(ProviderError::Silent(1)));
    }

    #[test]
    fn one_trailing_slash_of_the_base_url_is_removed() {
        let url_of = |base_url: &str| {
            OpenAiCompatible::new(Config {
                base_url: base_url.to_owned(),
                api_key: "key".to_owned(),
                model: "model".to_owned(),
                request_params: Map::new(),
                replays_reasoning: false,
            })
            .unwrap_or_else(|_| panic!("no client"))
            .url
        };
        assert_eq!(url_of("http://a/v1"), "http://a/v1/chat/completions");
        assert_eq!(url_of("http://a/v1/"), "http://a/v1/chat/completions");
        assert_eq!(url_of("http://a/v1//"), "http://a/v1//chat/completions");
    }
}
