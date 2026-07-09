use super::message_dto::{build_messages, translate_tools};
use super::sse::{TurnAccumulator, handle_event};
use super::wire::{MessagesRequest, OutputConfig, ThinkingConfig};
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::modules::provider::infrastructure::request::join_url;
use crate::modules::provider::infrastructure::streaming::{drain_sse, ensure_success};
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{Effort, Secret};

/// The Messages API version pin (the only value Anthropic currently accepts).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Required by the Messages API, unlike chat-completions. A model with a smaller cap surfaces a visible
/// `ProviderRejected` rather than silently truncating.
const MAX_OUTPUT_TOKENS: u32 = 16_000;

/// The manual `budget_tokens` shape 400s on the adaptive models, and vice versa, so the wire shape is
/// classified per model id — Kiri accepts any hand-typed Anthropic model, with no fixed catalog. An
/// unrecognized id falls through to `Budget` rather than guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnthropicThinkingMode {
    /// `thinking: {type: "enabled", budget_tokens, display: "summarized"}`; omission disables reasoning.
    Budget,
    /// `thinking: {type: "adaptive"}` + `output_config: {effort}`; omission disables reasoning.
    AdaptiveOptIn,
    /// The `AdaptiveOptIn` shape, but thinking defaults ON: disabling it needs an explicit `"disabled"`.
    AdaptiveDefaultOn,
}

impl AnthropicThinkingMode {
    fn classify(model: &str) -> Self {
        let model = model.to_ascii_lowercase();
        if model.contains("opus-4-8") || model.contains("opus-4-7") {
            Self::AdaptiveOptIn
        } else if model.contains("sonnet-5") {
            Self::AdaptiveDefaultOn
        } else {
            Self::Budget
        }
    }
}

/// Authenticates only with `x-api-key`: subscription OAuth is intentionally unsupported (ADR 0011).
pub struct AnthropicProvider {
    client: reqwest::Client,
    /// The host root. This adapter owns the whole `/v1/messages` path, unlike the OpenAI one whose base
    /// URL carries the `/v1` segment — including it here yields `/v1/v1/messages`.
    base_url: String,
    api_key: Secret,
    thinking: bool,
    effort: Effort,
}

impl AnthropicProvider {
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

    fn reasoning_enabled(&self) -> bool {
        self.thinking && self.effort != Effort::Off
    }

    /// Separate from `complete` so the wire shape is unit-testable without a network call.
    fn build_body<'a>(&self, request: &TurnRequest<'a>) -> MessagesRequest<'a> {
        let (system, messages) = build_messages(request.messages);
        let (thinking, output_config) = self.thinking_and_output_config(request.model);
        MessagesRequest {
            model: request.model,
            max_tokens: MAX_OUTPUT_TOKENS,
            stream: true,
            system,
            messages,
            tools: translate_tools(request.tools),
            thinking,
            output_config,
        }
    }

    /// Both the wire shape and the how-to-disable rule depend on the model, not only on whether
    /// reasoning is enabled — see [`AnthropicThinkingMode`].
    fn thinking_and_output_config(
        &self,
        model: &str,
    ) -> (Option<ThinkingConfig>, Option<OutputConfig>) {
        let mode = AnthropicThinkingMode::classify(model);
        if self.reasoning_enabled() {
            match mode {
                AnthropicThinkingMode::Budget => (
                    self.effort
                        .anthropic_budget_tokens()
                        .map(ThinkingConfig::enabled),
                    None,
                ),
                AnthropicThinkingMode::AdaptiveOptIn | AnthropicThinkingMode::AdaptiveDefaultOn => {
                    (
                        Some(ThinkingConfig::adaptive()),
                        self.effort
                            .as_anthropic_output_effort()
                            .map(|effort| OutputConfig { effort }),
                    )
                }
            }
        } else {
            match mode {
                AnthropicThinkingMode::AdaptiveDefaultOn => {
                    (Some(ThinkingConfig::disabled()), None)
                }
                AnthropicThinkingMode::Budget | AnthropicThinkingMode::AdaptiveOptIn => {
                    (None, None)
                }
            }
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

        let url = join_url(&self.base_url, "v1/messages");
        let response = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|error| AgentError::Provider(format!("failed to reach provider: {error}")))?;
        let response = ensure_success(response).await?;

        let mut accumulator = TurnAccumulator::default();
        drain_sse(response, |data| handle_event(data, &mut accumulator, sink)).await?;

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
            false,
            Effort::Off,
        )
    }

    fn provider_with_thinking(effort: Effort) -> AnthropicProvider {
        AnthropicProvider::new(
            reqwest::Client::new(),
            "https://api.anthropic.com",
            Secret::new("sk-ant-test"),
            true,
            effort,
        )
    }

    fn body_value_for_model(
        provider: &AnthropicProvider,
        messages: &[Message],
        tools: &[Value],
        model: &str,
    ) -> Value {
        let request = TurnRequest {
            messages,
            model,
            tools,
        };
        serde_json::to_value(provider.build_body(&request)).unwrap()
    }

    /// `AdaptiveOptIn` mode; the default for tests that do not care about the thinking wire shape.
    fn body_value(provider: &AnthropicProvider, messages: &[Message], tools: &[Value]) -> Value {
        body_value_for_model(provider, messages, tools, "claude-opus-4-8")
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

    // claude-haiku-4-5 is `Budget` mode.

    #[test]
    fn budget_mode_omits_thinking_when_disabled() {
        let value =
            body_value_for_model(&provider(), &[Message::user("hi")], &[], "claude-haiku-4-5");
        assert!(value.get("thinking").is_none());
    }

    #[test]
    fn budget_mode_omits_thinking_when_effort_is_off_even_with_thinking_enabled() {
        let value = body_value_for_model(
            &provider_with_thinking(Effort::Off),
            &[Message::user("hi")],
            &[],
            "claude-haiku-4-5",
        );
        assert!(value.get("thinking").is_none());
    }

    #[test]
    fn budget_mode_includes_thinking_with_the_effort_derived_budget_when_enabled() {
        let value = body_value_for_model(
            &provider_with_thinking(Effort::High),
            &[Message::user("hi")],
            &[],
            "claude-haiku-4-5",
        );
        assert_eq!(value["thinking"]["type"], "enabled");
        assert_eq!(
            value["thinking"]["budget_tokens"],
            Effort::High.anthropic_budget_tokens().unwrap()
        );
        assert_eq!(value["thinking"]["display"], "summarized");
        assert!(value.get("output_config").is_none());
    }

    // claude-opus-4-8 is `AdaptiveOptIn`.

    #[test]
    fn adaptive_opt_in_omits_thinking_when_disabled() {
        let value =
            body_value_for_model(&provider(), &[Message::user("hi")], &[], "claude-opus-4-8");
        assert!(value.get("thinking").is_none());
        assert!(value.get("output_config").is_none());
    }

    #[test]
    fn adaptive_opt_in_sends_adaptive_type_and_effort_when_enabled() {
        let value = body_value_for_model(
            &provider_with_thinking(Effort::Medium),
            &[Message::user("hi")],
            &[],
            "claude-opus-4-8",
        );
        assert_eq!(value["thinking"]["type"], "adaptive");
        assert!(value["thinking"].get("budget_tokens").is_none());
        assert_eq!(value["output_config"]["effort"], "medium");
    }

    // claude-sonnet-5 is `AdaptiveDefaultOn`: omitting the field would leave thinking running.

    #[test]
    fn adaptive_default_on_sends_explicit_disabled_when_thinking_flag_is_off() {
        let value =
            body_value_for_model(&provider(), &[Message::user("hi")], &[], "claude-sonnet-5");
        assert_eq!(value["thinking"]["type"], "disabled");
        assert!(value.get("output_config").is_none());
    }

    #[test]
    fn adaptive_default_on_sends_explicit_disabled_when_effort_is_off() {
        let value = body_value_for_model(
            &provider_with_thinking(Effort::Off),
            &[Message::user("hi")],
            &[],
            "claude-sonnet-5",
        );
        assert_eq!(value["thinking"]["type"], "disabled");
        assert!(value.get("output_config").is_none());
    }

    #[test]
    fn adaptive_default_on_sends_adaptive_type_and_effort_when_enabled() {
        let value = body_value_for_model(
            &provider_with_thinking(Effort::Xhigh),
            &[Message::user("hi")],
            &[],
            "claude-sonnet-5",
        );
        assert_eq!(value["thinking"]["type"], "adaptive");
        assert!(value["thinking"].get("budget_tokens").is_none());
        assert_eq!(value["output_config"]["effort"], "xhigh");
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

    /// A listener that accepts but never responds models a provider hanging after connect.
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
        let provider = AnthropicProvider::new(
            client,
            format!("http://{addr}"),
            Secret::new("k"),
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
