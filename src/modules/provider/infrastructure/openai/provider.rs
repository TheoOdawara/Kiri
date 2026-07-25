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
use crate::shared::kernel::provider::{
    Effort, NvidiaFamily, ProviderKind, Secret, ThinkingStyle, is_deepseek_model,
};

pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    /// `None` for a keyless local endpoint (Ollama / LM Studio).
    api_key: Option<Secret>,
    /// Selects the reasoning parameter: NVIDIA takes `chat_template_kwargs`, OpenAI `reasoning_effort`.
    kind: ProviderKind,
    thinking: bool,
    effort: Effort,
    /// Only meaningful for OpenAI-compatible / custom kinds; natives ignore it.
    thinking_style: ThinkingStyle,
}

impl OpenAiProvider {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Option<Secret>,
        kind: ProviderKind,
        thinking: bool,
        effort: Effort,
        thinking_style: ThinkingStyle,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key,
            kind,
            thinking,
            effort,
            thinking_style,
        }
    }

    fn reasoning_enabled(&self) -> bool {
        self.thinking && self.effort != Effort::Off
    }

    /// Resolve thinking-related body fields for the live model id (honors mid-session `/models`).
    fn thinking_fields(&self, model: &str) -> (Option<ChatTemplateKwargs>, Option<String>) {
        match self.kind {
            ProviderKind::Nvidia => {
                let kwargs = chat_template_kwargs(model, self.reasoning_enabled());
                (kwargs, None)
            }
            ProviderKind::Openai => (
                None,
                if self.reasoning_enabled() {
                    self.effort.as_openai_reasoning_effort().map(str::to_string)
                } else {
                    None
                },
            ),
            ProviderKind::OpenAiCompatible | ProviderKind::Custom => generalist_thinking_fields(
                self.thinking_style,
                model,
                self.reasoning_enabled(),
                self.effort,
            ),
            // Anthropic never reaches this adapter.
            ProviderKind::Anthropic => (None, None),
        }
    }
}

/// Family-keyed template kwargs. When `enabled` is false, still send an explicit `false` for known
/// families (many default thinking ON). `None` when the family has no confirmed convention.
fn chat_template_kwargs(model: &str, enabled: bool) -> Option<ChatTemplateKwargs> {
    match NvidiaFamily::classify(model) {
        NvidiaFamily::Nemotron | NvidiaFamily::Kimi => Some(ChatTemplateKwargs {
            thinking: Some(enabled),
            enable_thinking: None,
        }),
        NvidiaFamily::Qwen | NvidiaFamily::Glm | NvidiaFamily::Gemma => Some(ChatTemplateKwargs {
            thinking: None,
            enable_thinking: Some(enabled),
        }),
        NvidiaFamily::Other => None,
    }
}

/// Market generalist for OpenAI-compatible / custom endpoints. See
/// `docs/reference/model-thinking-parameters.md`.
fn generalist_thinking_fields(
    style: ThinkingStyle,
    model: &str,
    enabled: bool,
    effort: Effort,
) -> (Option<ChatTemplateKwargs>, Option<String>) {
    match style {
        ThinkingStyle::Off => (None, None),
        ThinkingStyle::ReasoningEffort => (
            None,
            if enabled {
                effort.as_openai_reasoning_effort().map(str::to_string)
            } else {
                None
            },
        ),
        ThinkingStyle::ChatTemplate => (chat_template_kwargs(model, enabled), None),
        ThinkingStyle::Auto => {
            if let Some(kwargs) = chat_template_kwargs(model, enabled) {
                return (Some(kwargs), None);
            }
            // DeepSeek: never invent kwargs / effort on auto (NIM hang / unreliable toggles).
            if is_deepseek_model(model) {
                return (None, None);
            }
            // GPT/Grok-like, or unknown ids when thinking was forced on (TOML) — lingua franca.
            if enabled {
                (
                    None,
                    effort.as_openai_reasoning_effort().map(str::to_string),
                )
            } else {
                (None, None)
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl CompletionProvider for OpenAiProvider {
    async fn complete(
        &self,
        request: TurnRequest<'_>,
        sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        let (chat_template_kwargs, reasoning_effort) = self.thinking_fields(request.model);
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
    use super::{OpenAiProvider, chat_template_kwargs, generalist_thinking_fields};
    use crate::modules::provider::application::completion_provider::{
        CompletionProvider, NullSink, TurnRequest,
    };
    use crate::modules::provider::infrastructure::openai::wire::ChatTemplateKwargs;
    use crate::modules::provider::infrastructure::test_support;
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::message::Message;
    use crate::shared::kernel::provider::{Effort, ProviderKind, Secret, ThinkingStyle};
    use std::time::Duration;

    fn provider(
        kind: ProviderKind,
        thinking: bool,
        effort: Effort,
        style: ThinkingStyle,
        base_url: String,
        api_key: Option<Secret>,
    ) -> OpenAiProvider {
        OpenAiProvider::new(
            reqwest::Client::builder().build().unwrap(),
            base_url,
            api_key,
            kind,
            thinking,
            effort,
            style,
        )
    }

    /// A listener that accepts but never answers models a provider hanging after connect: the regression
    /// where the first message did nothing, forever, with no error.
    #[tokio::test]
    async fn complete_fails_fast_when_the_provider_accepts_but_never_responds() {
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
        let provider = OpenAiProvider::new(
            client,
            format!("http://{addr}/v1"),
            Some(Secret::new("k")),
            ProviderKind::Nvidia,
            false,
            Effort::Off,
            ThinkingStyle::Auto,
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

        let provider = provider(
            ProviderKind::Nvidia,
            false,
            Effort::Off,
            ThinkingStyle::Auto,
            format!("http://{addr}/v1"),
            Some(Secret::new("k")),
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

    async fn capture_request(api_key: Option<Secret>) -> String {
        test_support::capture_request(|base_url| async move {
            let provider = provider(
                ProviderKind::Nvidia,
                false,
                Effort::Off,
                ThinkingStyle::Auto,
                base_url,
                api_key,
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

    #[tokio::test]
    async fn keyless_provider_omits_authorization_header() {
        let captured = capture_request(None).await;
        assert!(
            !captured.to_ascii_lowercase().contains("authorization"),
            "keyless request must omit Authorization; got:\n{captured}"
        );
    }

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
    fn chat_template_kwargs_sends_the_thinking_key_for_nemotron_and_kimi() {
        for model in [
            "nvidia/llama-3.3-nemotron-super-49b-v1",
            "moonshotai/kimi-k2",
        ] {
            assert!(
                matches!(
                    chat_template_kwargs(model, true),
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
    fn chat_template_kwargs_sends_the_enable_thinking_key_for_qwen_glm_and_gemma4() {
        for model in [
            "qwen/qwen3-235b-a22b",
            "zai-org/glm-4.5",
            "google/gemma-4-26b-a4b-it",
            "gemma4:26b",
        ] {
            assert!(
                matches!(
                    chat_template_kwargs(model, true),
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
    fn chat_template_kwargs_sends_nothing_for_unconfirmed_families() {
        for model in [
            "deepseek-ai/deepseek-v4",
            "deepseek-ai/deepseek-r1",
            "minimaxai/minimax-m1",
            "google/gemma-3-27b-it",
            "gemma",
        ] {
            assert!(
                chat_template_kwargs(model, true).is_none(),
                "expected no chat_template_kwargs for {model}"
            );
        }
    }

    #[test]
    fn generalist_auto_maps_gemma4_to_enable_thinking() {
        let (kwargs, effort) = generalist_thinking_fields(
            ThinkingStyle::Auto,
            "google/gemma-4-26b-a4b-it",
            true,
            Effort::High,
        );
        assert!(matches!(
            kwargs,
            Some(ChatTemplateKwargs {
                enable_thinking: Some(true),
                ..
            })
        ));
        assert!(effort.is_none());
    }

    #[test]
    fn chat_template_kwargs_sends_explicit_false_when_disabled() {
        assert!(matches!(
            chat_template_kwargs("qwen/qwen3", false),
            Some(ChatTemplateKwargs {
                thinking: None,
                enable_thinking: Some(false),
            })
        ));
    }

    #[test]
    fn generalist_auto_maps_qwen_to_kwargs_and_gpt_to_effort() {
        let (kwargs, effort) =
            generalist_thinking_fields(ThinkingStyle::Auto, "qwen3-32b", true, Effort::High);
        assert!(matches!(
            kwargs,
            Some(ChatTemplateKwargs {
                enable_thinking: Some(true),
                ..
            })
        ));
        assert!(effort.is_none());

        let (kwargs, effort) =
            generalist_thinking_fields(ThinkingStyle::Auto, "openai/gpt-5", true, Effort::Medium);
        assert!(kwargs.is_none());
        assert_eq!(effort.as_deref(), Some("medium"));
    }

    #[test]
    fn generalist_auto_sends_nothing_for_deepseek() {
        let (kwargs, effort) = generalist_thinking_fields(
            ThinkingStyle::Auto,
            "deepseek-ai/deepseek-r1",
            true,
            Effort::High,
        );
        assert!(kwargs.is_none());
        assert!(effort.is_none());
    }

    #[test]
    fn generalist_style_overrides() {
        let (kwargs, effort) =
            generalist_thinking_fields(ThinkingStyle::ReasoningEffort, "qwen3", true, Effort::Low);
        assert!(kwargs.is_none());
        assert_eq!(effort.as_deref(), Some("low"));

        let (kwargs, effort) =
            generalist_thinking_fields(ThinkingStyle::Off, "gpt-5", true, Effort::High);
        assert!(kwargs.is_none());
        assert!(effort.is_none());
    }

    #[tokio::test]
    async fn qwen_model_sends_enable_thinking_over_the_wire() {
        let captured = test_support::capture_request(|base_url| async move {
            let provider = provider(
                ProviderKind::Nvidia,
                true,
                Effort::High,
                ThinkingStyle::Auto,
                base_url,
                Some(Secret::new("k")),
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

    #[tokio::test]
    async fn deepseek_model_sends_no_chat_template_kwargs_over_the_wire() {
        let captured = test_support::capture_request(|base_url| async move {
            let provider = provider(
                ProviderKind::Nvidia,
                true,
                Effort::High,
                ThinkingStyle::Auto,
                base_url,
                Some(Secret::new("k")),
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

    #[tokio::test]
    async fn compatible_auto_sends_reasoning_effort_for_gpt() {
        let captured = test_support::capture_request(|base_url| async move {
            let provider = provider(
                ProviderKind::OpenAiCompatible,
                true,
                Effort::High,
                ThinkingStyle::Auto,
                base_url,
                Some(Secret::new("k")),
            );
            let messages = vec![Message::user("hi")];
            let request = TurnRequest {
                messages: &messages,
                model: "openai/gpt-5",
                tools: &[],
            };
            let mut sink = NullSink;
            let _ = provider.complete(request, &mut sink).await;
        })
        .await;
        assert!(
            captured.contains(r#""reasoning_effort":"high""#),
            "compatible GPT must send reasoning_effort; got:\n{captured}"
        );
    }
}
