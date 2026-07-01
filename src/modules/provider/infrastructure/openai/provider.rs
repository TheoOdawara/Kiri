use super::message_dto::WireMessage;
use super::sse::{TurnAccumulator, handle_event};
use super::wire::{ChatRequest, ChatTemplateKwargs};
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::modules::provider::infrastructure::request::{apply_optional_bearer, join_url};
use crate::modules::provider::infrastructure::streaming::{drain_sse, ensure_success};
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{Effort, NvidiaFamily, ProviderKind, Secret};

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
    /// The provider kind selects which thinking/reasoning parameter to send: NVIDIA uses
    /// `chat_template_kwargs`, OpenAI proper uses `reasoning_effort`, others send nothing.
    kind: ProviderKind,
    /// Whether thinking/reasoning is enabled for this provider. Gated further by `effort` — `Off`
    /// suppresses reasoning even when this is true.
    thinking: bool,
    /// The reasoning effort dial, mapped to vendor-specific parameters by `kind`.
    effort: Effort,
}

impl OpenAiProvider {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Option<Secret>,
        kind: ProviderKind,
        thinking: bool,
        effort: Effort,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key,
            kind,
            thinking,
            effort,
        }
    }

    /// Whether to ask the model to reason this turn: thinking must be enabled and effort must not be `Off`.
    fn reasoning_enabled(&self) -> bool {
        self.thinking && self.effort != Effort::Off
    }
}

/// The `chat_template_kwargs` NVIDIA's hosted model zoo expects for `model`'s family, or `None` for a
/// family with no confirmed reasoning-toggle convention (see `NvidiaFamily`). Keyed on the live turn's
/// model id (not a fixed profile field) so a mid-session `/models` switch is honored immediately.
fn nvidia_chat_template_kwargs(model: &str) -> Option<ChatTemplateKwargs> {
    match NvidiaFamily::classify(model) {
        NvidiaFamily::Nemotron | NvidiaFamily::Kimi => Some(ChatTemplateKwargs {
            thinking: Some(true),
            enable_thinking: None,
        }),
        NvidiaFamily::Qwen | NvidiaFamily::Glm => Some(ChatTemplateKwargs {
            thinking: None,
            enable_thinking: Some(true),
        }),
        NvidiaFamily::Other => None,
    }
}

#[async_trait::async_trait(?Send)]
impl CompletionProvider for OpenAiProvider {
    async fn complete(
        &self,
        request: TurnRequest<'_>,
        sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        let (chat_template_kwargs, reasoning_effort) = if self.reasoning_enabled() {
            match self.kind {
                ProviderKind::Nvidia => (nvidia_chat_template_kwargs(request.model), None),
                ProviderKind::Openai => (
                    None,
                    self.effort.as_openai_reasoning_effort().map(str::to_string),
                ),
                _ => (None, None),
            }
        } else {
            (None, None)
        };
        let body = ChatRequest {
            model: request.model,
            messages: request.messages.iter().map(WireMessage::from).collect(),
            stream: true,
            chat_template_kwargs,
            reasoning_effort,
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
    use super::{OpenAiProvider, nvidia_chat_template_kwargs};
    use crate::modules::provider::application::completion_provider::{
        CompletionProvider, NullSink, TurnRequest,
    };
    use crate::modules::provider::infrastructure::openai::wire::ChatTemplateKwargs;
    use crate::modules::provider::infrastructure::test_support;
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::message::Message;
    use crate::shared::kernel::provider::{Effort, ProviderKind, Secret};
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
            ProviderKind::Nvidia,
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
            ProviderKind::Nvidia,
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
            let provider = OpenAiProvider::new(
                client,
                base_url,
                api_key,
                ProviderKind::Nvidia,
                false,
                Effort::Off,
            );
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

    #[test]
    fn nvidia_chat_template_kwargs_sends_the_thinking_key_for_nemotron_and_kimi() {
        for model in [
            "nvidia/llama-3.3-nemotron-super-49b-v1",
            "moonshotai/kimi-k2",
        ] {
            assert!(
                matches!(
                    nvidia_chat_template_kwargs(model),
                    Some(ChatTemplateKwargs {
                        thinking: Some(true),
                        enable_thinking: None,
                    })
                ),
                "expected the `thinking` key for {model}"
            );
        }
    }

    #[test]
    fn nvidia_chat_template_kwargs_sends_the_enable_thinking_key_for_qwen_and_glm() {
        for model in ["qwen/qwen3-235b-a22b", "zai-org/glm-4.5"] {
            assert!(
                matches!(
                    nvidia_chat_template_kwargs(model),
                    Some(ChatTemplateKwargs {
                        thinking: None,
                        enable_thinking: Some(true),
                    })
                ),
                "expected the `enable_thinking` key for {model}"
            );
        }
    }

    #[test]
    fn nvidia_chat_template_kwargs_sends_nothing_for_unconfirmed_families() {
        for model in [
            "deepseek-ai/deepseek-v4",
            "deepseek-ai/deepseek-r1",
            "minimaxai/minimax-m1",
            "google/gemma-3-27b-it",
        ] {
            assert!(
                nvidia_chat_template_kwargs(model).is_none(),
                "expected no chat_template_kwargs for {model}"
            );
        }
    }

    /// End-to-end: a promoted family (Qwen) now sends its confirmed `enable_thinking` kwarg over the
    /// real request path (not just the unit-level helper above).
    #[tokio::test]
    async fn qwen_model_sends_enable_thinking_over_the_wire() {
        let captured = test_support::capture_request(|base_url| async move {
            let client = reqwest::Client::builder().build().unwrap();
            let provider = OpenAiProvider::new(
                client,
                base_url,
                Some(Secret::new("k")),
                ProviderKind::Nvidia,
                true,
                Effort::High,
            );
            let messages = vec![Message::user("hi")];
            let request = TurnRequest {
                messages: &messages,
                model: "qwen/qwen3-235b-a22b",
                tools: &[],
            };
            let mut sink = NullSink;
            let _ = provider.complete(request, &mut sink).await;
        })
        .await;
        assert!(
            captured.contains(r#""enable_thinking":true"#),
            "Qwen must send enable_thinking:true; got:\n{captured}"
        );
        assert!(
            !captured.contains(r#""thinking":true"#),
            "Qwen must not send the Nemotron/Kimi thinking key; got:\n{captured}"
        );
    }

    /// End-to-end: DeepSeek stays unsupported (see `NvidiaFamily::Other`'s doc comment for why) and must
    /// send no `chat_template_kwargs` at all over the real request path.
    #[tokio::test]
    async fn deepseek_model_sends_no_chat_template_kwargs_over_the_wire() {
        let captured = test_support::capture_request(|base_url| async move {
            let client = reqwest::Client::builder().build().unwrap();
            let provider = OpenAiProvider::new(
                client,
                base_url,
                Some(Secret::new("k")),
                ProviderKind::Nvidia,
                true,
                Effort::High,
            );
            let messages = vec![Message::user("hi")];
            let request = TurnRequest {
                messages: &messages,
                model: "deepseek-ai/deepseek-v4",
                tools: &[],
            };
            let mut sink = NullSink;
            let _ = provider.complete(request, &mut sink).await;
        })
        .await;
        assert!(
            !captured.contains("chat_template_kwargs"),
            "an unsupported family must send no chat_template_kwargs at all; got:\n{captured}"
        );
    }
}
