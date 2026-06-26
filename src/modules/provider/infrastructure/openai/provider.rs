use eventsource_stream::Eventsource;
use tokio_stream::StreamExt;

use super::message_dto::MessageDto;
use super::sse::{TurnAccumulator, handle_event};
use super::wire::{ChatRequest, ChatTemplateKwargs};
use crate::modules::agent::domain::completed_turn::CompletedTurn;
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Effort;

/// Cap a provider error body before it reaches the transcript. The body can reflect the request we
/// sent (which may include file contents the agent read), so it is bounded to a short preview rather
/// than surfaced in full.
const MAX_ERROR_BODY_CHARS: usize = 600;

fn truncate_body(body: String) -> String {
    if body.chars().count() <= MAX_ERROR_BODY_CHARS {
        return body;
    }
    let head: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
    format!("{head}… (truncated)")
}

/// OpenAI-compatible chat provider (NVIDIA today). Holds the HTTP client and endpoint/credentials;
/// translates a domain `TurnRequest` into the wire `ChatRequest`, streams the response forwarding
/// deltas to `sink`, and assembles the turn.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Whether the model accepts `chat_template_kwargs.thinking` at all; off for one that rejects/stalls
    /// on it. Gated further by `effort` — `Effort::Off` suppresses reasoning even when this is true.
    thinking: bool,
    /// The reasoning effort dial. This OpenAI-compatible adapter maps it to the on/off `thinking` kwarg
    /// (the only reasoning control NVIDIA's nemotron template exposes); richer per-vendor mapping lives
    /// in the vendor-specific adapters.
    effort: Effort,
}

impl OpenAiProvider {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        thinking: bool,
        effort: Effort,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
            thinking,
            effort,
        }
    }

    /// Whether to ask the model to reason this turn: the model must accept the kwarg and effort must not
    /// be `Off`.
    fn reasoning_enabled(&self) -> bool {
        self.thinking && self.effort != Effort::Off
    }
}

#[async_trait::async_trait(?Send)]
impl CompletionProvider for OpenAiProvider {
    async fn complete(
        &self,
        request: TurnRequest<'_>,
        sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        let body = ChatRequest {
            model: request.model,
            messages: request.messages.iter().map(MessageDto::from).collect(),
            stream: true,
            chat_template_kwargs: self
                .reasoning_enabled()
                .then_some(ChatTemplateKwargs { thinking: true }),
            tools: request.tools,
        };

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| AgentError::Provider(format!("failed to reach provider: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            // The status drives the error path; the body is only diagnostic. If reading it fails, keep
            // the failure visible in the message rather than silently blanking it.
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<error body unavailable: {error}>"));
            // A 4xx means the body we sent is unacceptable; resending it unchanged fails identically,
            // so it is surfaced distinctly to let the frontend drop the offending turn. 5xx/other stay
            // a plain (transient) provider error.
            return Err(if status.is_client_error() {
                AgentError::ProviderRejected {
                    status: status.as_u16(),
                    body: truncate_body(body),
                }
            } else {
                AgentError::Provider(format!(
                    "provider returned {status}: {}",
                    truncate_body(body)
                ))
            });
        }

        let mut accumulator = TurnAccumulator::default();
        let stream = response.bytes_stream().eventsource();
        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            let event = event
                .map_err(|error| AgentError::Provider(format!("error reading stream: {error}")))?;
            handle_event(&event.data, &mut accumulator, sink)?;
        }

        Ok(accumulator.into_completed())
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ERROR_BODY_CHARS, OpenAiProvider, truncate_body};
    use crate::modules::agent::domain::message::Message;
    use crate::modules::agent::domain::stream_event::StreamEvent;
    use crate::modules::provider::application::completion_provider::{
        CompletionProvider, EventSink, TurnRequest,
    };
    use crate::shared::kernel::error::AgentError;
    use std::time::Duration;

    #[test]
    fn truncate_body_keeps_short_bodies_verbatim() {
        let short = "invalid model".to_string();
        assert_eq!(truncate_body(short.clone()), short);
    }

    #[test]
    fn truncate_body_caps_long_bodies() {
        let out = truncate_body("x".repeat(5_000));
        assert!(out.ends_with("… (truncated)"));
        assert!(out.chars().count() <= MAX_ERROR_BODY_CHARS + 16);
    }

    struct NullSink;
    impl EventSink for NullSink {
        fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
            Ok(())
        }
    }

    /// Run-to-verify of the timeout fix: a local listener that ACCEPTS the connection but never sends a
    /// byte models a provider that hangs after connect — the reported "first message does nothing, no
    /// error". `read_timeout` must make `complete()` fail fast instead of hanging forever. Hermetic
    /// (loopback only), bounded well under the outer guard.
    #[tokio::test]
    async fn complete_fails_fast_when_the_provider_accepts_but_never_responds() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Hold every accepted connection open without ever responding.
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(300))
            .build()
            .unwrap();
        let provider = OpenAiProvider::new(
            client,
            format!("http://{addr}/v1"),
            "k",
            false,
            crate::shared::kernel::provider::Effort::Off,
        );

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

    /// A 4xx means the body we sent is unacceptable; it must surface as `ProviderRejected` carrying the
    /// status and body so the frontend can drop the offending turn instead of resending it forever.
    #[tokio::test]
    async fn complete_surfaces_a_client_error_as_provider_rejected() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await; // drain the request before replying
                let body = "invalid model: nope";
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            }
        });

        let client = reqwest::Client::builder().build().unwrap();
        let provider = OpenAiProvider::new(
            client,
            format!("http://{addr}/v1"),
            "k",
            false,
            crate::shared::kernel::provider::Effort::Off,
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
                assert!(body.contains("invalid model"), "body lost: {body:?}");
            }
            other => panic!("expected ProviderRejected(400), got {other:?}"),
        }
    }
}
