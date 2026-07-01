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

/// Per-turn output cap. `max_tokens` is required by the Messages API (unlike chat-completions). Set
/// generously for an agent that may emit large tool inputs (file writes) while staying within the
/// flagship models' output limits; a model with a smaller cap surfaces a visible `ProviderRejected`.
const MAX_OUTPUT_TOKENS: u32 = 16_000;

/// Which `thinking` wire shape a model accepts. Confirmed against Anthropic's current docs: manual
/// `type: "enabled"` + `budget_tokens` is rejected with a 400 on Claude Sonnet 5 and Opus 4.8/4.7, which
/// require `type: "adaptive"` (+ a top-level `output_config.effort`) instead — Opus 4.8/4.7 default
/// thinking OFF (omitting `thinking` suffices to disable), Sonnet 5 defaults it ON (disabling requires an
/// explicit `type: "disabled"`, not omission). Only Claude Haiku 4.5 and older Claude 4 models still use
/// the manual budget shape. Classified by model id substring, not `ProviderKind` alone, since Kiri lets a
/// user type any Anthropic model id (no fixed catalog) and the wire shape genuinely differs per model.
///
/// Out of scope: Claude Fable 5 / "Claude Mythos" models are adaptive-only/always-on per the docs, but
/// are not part of Kiri's confirmed lineup (Sonnet 5 / Haiku 4.5 / Opus 4.8) — a hand-typed model id from
/// that family falls through to `Budget` and 400s until a future pass confirms and adds it, mirroring the
/// same "don't guess, extend once confirmed" rule `NvidiaFamily` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnthropicThinkingMode {
    /// `thinking: {type: "enabled", budget_tokens, display: "summarized"}`. Omitting `thinking` disables
    /// reasoning (unchanged from before this classifier existed).
    Budget,
    /// `thinking: {type: "adaptive"}` + `output_config: {effort}`. Thinking defaults OFF; omitting
    /// `thinking` already disables it.
    AdaptiveOptIn,
    /// Same wire shape as `AdaptiveOptIn`, but thinking defaults ON: disabling it requires explicitly
    /// sending `thinking: {type: "disabled"}` rather than omitting the field.
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

/// Anthropic Messages API provider (API key). Holds the HTTP client, endpoint and key; translates a
/// domain `TurnRequest` into the Messages wire shape, streams the response forwarding deltas to `sink`,
/// and assembles the turn. Subscription OAuth is intentionally unsupported (see the provider-auth ADR);
/// this adapter authenticates only with `x-api-key`.
///
/// `base_url` is the host root (default `https://api.anthropic.com`) — the adapter owns the full
/// `/v1/messages` path, unlike the OpenAI adapter where the `/v1` segment lives in the base URL. Do not
/// include `/v1` in an Anthropic base URL or it becomes `/v1/v1/messages`.
///
/// Extended thinking: when `thinking` is enabled and `effort` is not `Off`, the request carries whichever
/// shape `AnthropicThinkingMode::classify` picks for the turn's model (manual `budget_tokens` or adaptive
/// `effort`). The returned `thinking`/`redacted_thinking` block (and its `signature`, streamed via
/// `signature_delta`) is preserved on the domain `Message`/`CompletedTurn` (see `ThinkingBlock`) and
/// resent ahead of the `tool_use` block on the following turn — the Messages API round-trip requirement
/// this adapter used to defer entirely (see `docs/decisions/0011-provider-agnostic-by-api-key.md`).
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    /// Held as a `Secret` (zeroized on drop, redacted in Debug) rather than a plain `String`, exposed
    /// only at the `x-api-key` header call site.
    api_key: Secret,
    /// Whether extended thinking is enabled for this provider. Gated further by `effort` — `Off`
    /// suppresses it even when this is true (mirrors `OpenAiProvider::reasoning_enabled`).
    thinking: bool,
    /// The reasoning effort dial, mapped to a `budget_tokens` value via `Effort::anthropic_budget_tokens`.
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

    /// Whether to ask the model to reason this turn: thinking must be enabled and effort must not be
    /// `Off` (mirrors `OpenAiProvider::reasoning_enabled`).
    fn reasoning_enabled(&self) -> bool {
        self.thinking && self.effort != Effort::Off
    }

    /// Build the request body for a turn. Kept separate from `complete` so the wire shape (system
    /// lifting, tool translation) is unit-testable without a network call.
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

    /// The `thinking`/`output_config` pair for this turn's model, per `AnthropicThinkingMode`. See the
    /// mode enum's doc comment for why the wire shape and the "how do I turn it off" rule both depend on
    /// the model, not just whether reasoning is enabled.
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

    /// `claude-opus-4-8` is `AdaptiveOptIn` (see `AnthropicThinkingMode`); used as the default model for
    /// tests that don't care about the thinking wire shape.
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

    // claude-haiku-4-5 is `Budget` mode: the manual `type:"enabled"`+`budget_tokens` shape.

    #[test]
    fn budget_mode_omits_thinking_when_disabled() {
        let value =
            body_value_for_model(&provider(), &[Message::user("hi")], &[], "claude-haiku-4-5");
        assert!(value.get("thinking").is_none());
    }

    #[test]
    fn budget_mode_omits_thinking_when_effort_is_off_even_with_thinking_enabled() {
        // Mirrors OpenAiProvider::reasoning_enabled: thinking=true is gated by effort != Off.
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

    // claude-opus-4-8 is `AdaptiveOptIn`: thinking defaults off, omitting `thinking` suffices to disable.

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

    // claude-sonnet-5 is `AdaptiveDefaultOn`: thinking defaults ON, so disabling it requires an explicit
    // `{type:"disabled"}` — omitting the field would leave adaptive thinking running.

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
