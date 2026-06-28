use super::message_dto::MessageDto;
use super::sse::{TurnAccumulator, handle_event};
use super::wire::{ChatRequest, ChatTemplateKwargs};
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::modules::provider::infrastructure::request::{apply_optional_bearer, join_url};
use crate::modules::provider::infrastructure::streaming::{drain_sse, ensure_success};
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{Effort, Secret};

/// OpenAI-compatible chat provider (NVIDIA / OpenAI / compatible / custom / keyless local). Holds the
/// HTTP client and endpoint/credentials; translates a domain `TurnRequest` into the wire `ChatRequest`,
/// streams the response forwarding deltas to `sink`, and assembles the turn.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    /// Optional API key. `Some` for an authenticated endpoint (held as a `Secret`: zeroized on drop,
    /// redacted in Debug, exposed only at the auth-header site). `None` for a keyless local endpoint
    /// (Ollama / LM Studio), in which case `complete` omits the `Authorization` header entirely.
    api_key: Option<Secret>,
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
        api_key: Option<Secret>,
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

        let url = join_url(&self.base_url, "chat/completions");
        let request = apply_optional_bearer(self.client.post(&url), &self.api_key);
        let response =
            request.json(&body).send().await.map_err(|error| {
                AgentError::Provider(format!("failed to reach provider: {error}"))
            })?;
        let response = ensure_success(response).await?;

        let mut accumulator = TurnAccumulator::default();
        drain_sse(response, |data| handle_event(data, &mut accumulator, sink)).await?;

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
        CompletionProvider, NullSink, TurnRequest,
    };
    use crate::modules::provider::infrastructure::test_support;
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::message::Message;
    use crate::shared::kernel::provider::{Effort, Secret};
    use std::time::Duration;

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
            Some(crate::shared::kernel::provider::Secret::new("k")),
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
            Some(crate::shared::kernel::provider::Secret::new("k")),
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

    /// Drive one `complete` (keyed or keyless) against the shared loopback capture server and return the
    /// raw request bytes it sent — the assertion is on what was sent, not the response.
    async fn capture_request(api_key: Option<Secret>) -> String {
        test_support::capture_request(|base_url| async move {
            let client = reqwest::Client::builder().build().unwrap();
            let provider = OpenAiProvider::new(client, base_url, api_key, false, Effort::Off);
            let messages = vec![Message::user("hi")];
            let request = TurnRequest {
                messages: &messages,
                model: "m",
                tools: &[],
            };
            let mut sink = NullSink;
            let _ = provider.complete(request, &mut sink).await;
        })
        .await
    }

    /// Regression lock for the LM Studio / Ollama fix: a keyless adapter must send NO `Authorization`
    /// header — not even `Bearer ` (empty), which some local servers reject.
    #[tokio::test]
    async fn keyless_provider_omits_authorization_header() {
        let captured = capture_request(None).await;
        assert!(
            !captured.to_ascii_lowercase().contains("authorization"),
            "keyless request must omit Authorization; got:\n{captured}"
        );
    }

    /// The dual: a keyed adapter must send `Authorization: Bearer <key>`.
    #[tokio::test]
    async fn keyed_provider_sends_bearer_authorization() {
        let captured = capture_request(Some(Secret::new("k"))).await;
        assert!(
            captured
                .to_ascii_lowercase()
                .contains("authorization: bearer k"),
            "keyed request must send Bearer; got:\n{captured}"
        );
    }
}
