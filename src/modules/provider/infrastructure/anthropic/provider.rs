use eventsource_stream::Eventsource;
use tokio_stream::StreamExt;

use super::message_dto::{build_messages, translate_tools};
use super::sse::{TurnAccumulator, handle_event};
use super::wire::MessagesRequest;
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::modules::provider::infrastructure::http_error::error_from_status;
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Secret;

/// The Messages API version pin (the only value Anthropic currently accepts).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Per-turn output cap. `max_tokens` is required by the Messages API (unlike chat-completions). Set
/// generously for an agent that may emit large tool inputs (file writes) while staying within the
/// flagship models' output limits; a model with a smaller cap surfaces a visible `ProviderRejected`.
const MAX_OUTPUT_TOKENS: u32 = 16_000;

/// Anthropic Messages API provider (API key). Holds the HTTP client, endpoint and key; translates a
/// domain `TurnRequest` into the Messages wire shape, streams the response forwarding deltas to `sink`,
/// and assembles the turn. Subscription OAuth is intentionally unsupported (see the provider-auth ADR);
/// this adapter authenticates only with `x-api-key`.
///
/// `base_url` is the host root (default `https://api.anthropic.com`) — the adapter owns the full
/// `/v1/messages` path, unlike the OpenAI adapter where the `/v1` segment lives in the base URL. Do not
/// include `/v1` in an Anthropic base URL or it becomes `/v1/v1/messages`.
///
/// Extended thinking is deliberately NOT enabled here. With thinking on, the Messages API requires the
/// `thinking` block (and its `signature`, streamed via `signature_delta`) to be preserved and resent
/// ahead of the `tool_use` block on the following turn; the domain `Message`/`CompletedTurn` do not
/// model provider reasoning traces, so replaying them needs a domain change and live verification. Until
/// that lands (a tracked follow-up), this adapter sends no `thinking`/`output_config`, which keeps the
/// tool-use round-trips this agentic harness relies on correct on the opt-in-thinking Claude models
/// (Opus 4.8 / Sonnet 4.6 / Haiku 4.5). The `effort` dial therefore does not affect Claude yet.
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    /// Held as a `Secret` (zeroized on drop, redacted in Debug) rather than a plain `String`, exposed
    /// only at the `x-api-key` header call site.
    api_key: Secret,
}

impl AnthropicProvider {
    pub fn new(client: reqwest::Client, base_url: impl Into<String>, api_key: Secret) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key,
        }
    }

    /// Build the request body for a turn. Kept separate from `complete` so the wire shape (system
    /// lifting, tool translation) is unit-testable without a network call.
    fn build_body<'a>(&self, request: &TurnRequest<'a>) -> MessagesRequest<'a> {
        let (system, messages) = build_messages(request.messages);
        MessagesRequest {
            model: request.model,
            max_tokens: MAX_OUTPUT_TOKENS,
            stream: true,
            system,
            messages,
            tools: translate_tools(request.tools),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl CompletionProvider for AnthropicProvider {
    async fn complete(
        &self,
        request: TurnRequest<'_>,
        sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        let body = self.build_body(&request);

        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|error| AgentError::Provider(format!("failed to reach provider: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<error body unavailable: {error}>"));
            return Err(error_from_status(status, body));
        }

        let mut accumulator = TurnAccumulator::default();
        let stream = response.bytes_stream().eventsource();
        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            let event = event
                .map_err(|error| AgentError::Provider(format!("error reading stream: {error}")))?;
            handle_event(&event.data, &mut accumulator, sink)?;
        }

        // The output cap (max_tokens) truncated the turn before any content or tool call: surface it
        // rather than returning an empty turn with no feedback.
        if accumulator.hit_empty_output_limit() {
            return Err(AgentError::Provider(format!(
                "the model hit the {MAX_OUTPUT_TOKENS}-token output cap before producing a response; \
                 shorten the request"
            )));
        }
        Ok(accumulator.into_completed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::provider::application::completion_provider::NullSink;
    use crate::shared::kernel::message::Message;
    use serde_json::{Value, json};
    use std::time::Duration;

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new(
            reqwest::Client::new(),
            "https://api.anthropic.com",
            Secret::new("sk-ant-test"),
        )
    }

    fn body_value(provider: &AnthropicProvider, messages: &[Message], tools: &[Value]) -> Value {
        let request = TurnRequest {
            messages,
            model: "claude-opus-4-8",
            tools,
        };
        serde_json::to_value(provider.build_body(&request)).unwrap()
    }

    #[test]
    fn body_has_required_fields_and_lifts_system() {
        let messages = vec![Message::system("be terse"), Message::user("hi")];
        let value = body_value(&provider(), &messages, &[]);
        assert_eq!(value["model"], "claude-opus-4-8");
        assert_eq!(value["max_tokens"], MAX_OUTPUT_TOKENS);
        assert_eq!(value["stream"], true);
        assert_eq!(value["system"], "be terse");
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn body_omits_extended_thinking_and_output_config() {
        // Extended thinking is deliberately not enabled (it would require replaying thinking blocks +
        // signatures across tool-use turns, which the domain does not model yet). The body must carry
        // neither field so a default Claude session's tool-use round-trips stay correct.
        let value = body_value(&provider(), &[Message::user("hi")], &[]);
        assert!(value.get("thinking").is_none());
        assert!(value.get("output_config").is_none());
    }

    #[test]
    fn tools_are_translated_and_omitted_when_empty() {
        let empty = body_value(&provider(), &[Message::user("hi")], &[]);
        assert!(empty.get("tools").is_none());

        let tools = vec![json!({
            "type": "function",
            "function": {"name": "read_file", "description": "d", "parameters": {"type": "object"}}
        })];
        let with_tools = body_value(&provider(), &[Message::user("hi")], &tools);
        assert_eq!(with_tools["tools"][0]["name"], "read_file");
        assert_eq!(with_tools["tools"][0]["input_schema"]["type"], "object");
    }

    /// A 4xx from the Messages API (e.g. an unknown model, an over-cap `max_tokens`) must surface as
    /// `ProviderRejected` carrying the status and body so the frontend can drop the offending turn.
    #[tokio::test]
    async fn complete_surfaces_a_client_error_as_provider_rejected() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#;
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            }
        });

        let provider = AnthropicProvider::new(
            reqwest::Client::new(),
            format!("http://{addr}"),
            Secret::new("k"),
        );
        let messages = vec![Message::user("hi")];
        let request = TurnRequest {
            messages: &messages,
            model: "m",
            tools: &[],
        };
        let mut sink = NullSink;

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            provider.complete(request, &mut sink),
        )
        .await
        .expect("provider should not hang");
        match result {
            Err(AgentError::ProviderRejected { status, body }) => {
                assert_eq!(status, 400);
                assert!(body.contains("bad model"), "body lost: {body:?}");
            }
            other => panic!("expected ProviderRejected(400), got {other:?}"),
        }
    }

    /// Run-to-verify of the shared read-timeout: a listener that accepts but never responds must make
    /// `complete()` fail fast rather than hang.
    #[tokio::test]
    async fn complete_fails_fast_when_the_provider_never_responds() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(300))
            .build()
            .unwrap();
        let provider = AnthropicProvider::new(client, format!("http://{addr}"), Secret::new("k"));
        let messages = vec![Message::user("hi")];
        let request = TurnRequest {
            messages: &messages,
            model: "m",
            tools: &[],
        };
        let mut sink = NullSink;

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            provider.complete(request, &mut sink),
        )
        .await;
        match outcome {
            Err(_) => panic!("provider hung past 5s — the read timeout regressed"),
            Ok(Ok(_)) => panic!("expected an error from a non-responding provider, got a turn"),
            Ok(Err(_)) => {}
        }
    }
}
