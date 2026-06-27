use eventsource_stream::Eventsource;
use tokio_stream::StreamExt;

use super::message_dto::MessageDto;
use super::sse::{TurnAccumulator, handle_event};
use super::wire::{ChatRequest, ChatTemplateKwargs};
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::modules::provider::infrastructure::http_error::error_from_status;
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{Effort, Secret};

/// OpenAI-compatible chat provider (NVIDIA today). Holds the HTTP client and endpoint/credentials;
/// translates a domain `TurnRequest` into the wire `ChatRequest`, streams the response forwarding
/// deltas to `sink`, and assembles the turn.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    /// Held as a `Secret` (zeroized on drop, redacted in Debug) rather than a plain `String`, so the
    /// key bytes are wiped when the adapter is dropped on a live `/provider` swap; exposed only at the
    /// auth-header call site.
    api_key: Secret,
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
        api_key: Secret,
        thinking: bool,
        effort: Effort,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key,
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
            .bearer_auth(self.api_key.expose())
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

        // A turn the output token cap truncated before any content or tool call: surface it instead of
        // returning an empty turn the user gets no feedback on.
        if accumulator.hit_empty_output_limit() {
            return Err(AgentError::Provider(
                "the provider hit the output token limit before producing a response; \
                 raise the model's max output tokens or shorten the request"
                    .to_string(),
            ));
        }
        Ok(accumulator.into_completed())
    }
}

#[cfg(test)]
mod tests {
    use super::OpenAiProvider;
    use crate::modules::provider::application::completion_provider::{
        CompletionProvider, EventSink, TurnRequest,
    };
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::message::Message;
    use crate::shared::kernel::stream_event::StreamEvent;
    use std::time::Duration;

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
            crate::shared::kernel::provider::Secret::new("k"),
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
            crate::shared::kernel::provider::Secret::new("k"),
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
