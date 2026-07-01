//! Pure wire DTOs for the Anthropic Messages API: the request body (serialize) and the streamed event
//! payloads (deserialize). The domain→wire message/tool translation lives in `message_dto`; the
//! stream assembly in `sse`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message_dto::AnthropicMessage;

/// The Anthropic Messages API request body. `system` is a top-level field (not a message); `messages`
/// are the alternating user/assistant turns built by `message_dto`; `tools` are the Anthropic-shaped
/// schemas translated from the registry's OpenAI shape. `thinking` is only present when extended
/// thinking is enabled for this turn; `output_config` carries the adaptive-mode `effort` dial, only
/// present alongside `thinking: {type: "adaptive"}` (see the extended-thinking note on
/// `AnthropicProvider`/`AnthropicThinkingMode`).
#[derive(Debug, Serialize)]
pub(crate) struct MessagesRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

/// The `thinking` request field, tri-state per `AnthropicThinkingMode`:
/// - `enabled(budget_tokens)` — the manual, budget-driven shape (`display: "summarized"` set explicitly
///   so the returned `thinking` text is never silently empty regardless of a model's own default).
/// - `adaptive()` — lets the model decide whether/how much to think; paired with `output_config.effort`.
/// - `disabled()` — explicitly turns thinking off; required (not just omission) on a model whose
///   adaptive thinking defaults on (`AnthropicThinkingMode::AdaptiveDefaultOn`).
#[derive(Debug, Serialize)]
pub(crate) struct ThinkingConfig {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<&'static str>,
}

impl ThinkingConfig {
    pub fn enabled(budget_tokens: u32) -> Self {
        Self {
            kind: "enabled",
            budget_tokens: Some(budget_tokens),
            display: Some("summarized"),
        }
    }

    pub fn adaptive() -> Self {
        Self {
            kind: "adaptive",
            budget_tokens: None,
            display: None,
        }
    }

    pub fn disabled() -> Self {
        Self {
            kind: "disabled",
            budget_tokens: None,
            display: None,
        }
    }
}

/// The adaptive-thinking `effort` dial (`"low"|"medium"|"high"|"xhigh"|"max"`), sent alongside
/// `thinking: {type: "adaptive"}`. See `Effort::as_anthropic_output_effort`.
#[derive(Debug, Serialize)]
pub(crate) struct OutputConfig {
    pub effort: &'static str,
}

/// One streamed Server-Sent Event from the Messages API, dispatched on its `type` discriminator. Only
/// the events that carry assembly-relevant data are modeled; everything else (`message_start`,
/// `content_block_stop`, `message_delta`, `message_stop`, `ping`, future types) falls into `Other` and
/// is ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WireStreamEvent {
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockStart,
    },
    ContentBlockDelta {
        index: u32,
        delta: BlockDelta,
    },
    /// The end-of-message delta, carrying `stop_reason` (`"max_tokens"` means the output cap truncated
    /// the turn). Modeled so a silent truncation can be surfaced.
    MessageDelta {
        delta: MessageDelta,
    },
    /// An in-stream error the API can deliver on an otherwise-200 SSE response.
    Error {
        error: ApiError,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageDelta {
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// The opening descriptor of a content block. `tool_use` carries its id + name; `redacted_thinking`
/// arrives whole here (an opaque encrypted blob, unlike `thinking`, which streams incrementally via
/// `thinking_delta`/`signature_delta` — there is no delta kind for a redacted block). Text/thinking
/// blocks otherwise fall into `Other`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlockStart {
    ToolUse {
        id: String,
        name: String,
    },
    RedactedThinking {
        data: String,
    },
    #[serde(other)]
    Other,
}

/// An incremental update to a content block. `text_delta` is answer content, `thinking_delta` is
/// reasoning, `signature_delta` is the cryptographic signature of a thinking block (must be replayed
/// byte-for-byte ahead of any `tool_use` block on a later turn), `input_json_delta` is a slice of a tool
/// call's JSON input; any other delta kind is ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum BlockDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiError {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_block_start_reads_tool_use_id_and_name() {
        let event: WireStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}"#,
        )
        .unwrap();
        match event {
            WireStreamEvent::ContentBlockStart {
                index,
                content_block: ContentBlockStart::ToolUse { id, name },
            } => {
                assert_eq!(index, 1);
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "read_file");
            }
            other => panic!("expected a tool_use start, got {other:?}"),
        }
    }

    #[test]
    fn content_block_start_reads_redacted_thinking_data_whole() {
        let event: WireStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"encrypted-blob"}}"#,
        )
        .unwrap();
        match event {
            WireStreamEvent::ContentBlockStart {
                index,
                content_block: ContentBlockStart::RedactedThinking { data },
            } => {
                assert_eq!(index, 0);
                assert_eq!(data, "encrypted-blob");
            }
            other => panic!("expected a redacted_thinking start, got {other:?}"),
        }
    }

    #[test]
    fn text_block_start_falls_into_other() {
        let event: WireStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        )
        .unwrap();
        assert!(matches!(
            event,
            WireStreamEvent::ContentBlockStart {
                content_block: ContentBlockStart::Other,
                ..
            }
        ));
    }

    #[test]
    fn deltas_deserialize_by_kind() {
        let text: WireStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
        )
        .unwrap();
        assert!(matches!(
            text,
            WireStreamEvent::ContentBlockDelta {
                delta: BlockDelta::TextDelta { text },
                ..
            } if text == "Hi"
        ));

        let json: WireStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"p\""}}"#,
        )
        .unwrap();
        assert!(matches!(
            json,
            WireStreamEvent::ContentBlockDelta {
                delta: BlockDelta::InputJsonDelta { partial_json },
                ..
            } if partial_json == "{\"p\""
        ));

        let signature: WireStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
        )
        .unwrap();
        assert!(matches!(
            signature,
            WireStreamEvent::ContentBlockDelta {
                delta: BlockDelta::SignatureDelta { signature },
                ..
            } if signature == "sig"
        ));
    }

    fn base_request(thinking: Option<ThinkingConfig>) -> MessagesRequest<'static> {
        MessagesRequest {
            model: "claude-opus-4-8",
            max_tokens: 16_000,
            stream: true,
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
            thinking,
            output_config: None,
        }
    }

    #[test]
    fn messages_request_serializes_thinking_when_present() {
        let value =
            serde_json::to_value(base_request(Some(ThinkingConfig::enabled(8_192)))).unwrap();
        assert_eq!(value["thinking"]["type"], "enabled");
        assert_eq!(value["thinking"]["budget_tokens"], 8_192);
        assert_eq!(value["thinking"]["display"], "summarized");
    }

    #[test]
    fn messages_request_omits_thinking_when_absent() {
        let value = serde_json::to_value(base_request(None)).unwrap();
        assert!(value.get("thinking").is_none());
    }

    #[test]
    fn thinking_config_adaptive_omits_budget_and_display() {
        let value = serde_json::to_value(base_request(Some(ThinkingConfig::adaptive()))).unwrap();
        assert_eq!(value["thinking"]["type"], "adaptive");
        assert!(value["thinking"].get("budget_tokens").is_none());
        assert!(value["thinking"].get("display").is_none());
    }

    #[test]
    fn thinking_config_disabled_omits_budget_and_display() {
        let value = serde_json::to_value(base_request(Some(ThinkingConfig::disabled()))).unwrap();
        assert_eq!(value["thinking"]["type"], "disabled");
        assert!(value["thinking"].get("budget_tokens").is_none());
        assert!(value["thinking"].get("display").is_none());
    }

    #[test]
    fn output_config_serializes_effort_and_is_omitted_when_absent() {
        let mut request = base_request(Some(ThinkingConfig::adaptive()));
        request.output_config = Some(OutputConfig { effort: "medium" });
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["output_config"]["effort"], "medium");

        let without = base_request(None);
        let value = serde_json::to_value(without).unwrap();
        assert!(value.get("output_config").is_none());
    }

    #[test]
    fn message_lifecycle_events_are_ignored() {
        for raw in [
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
            r#"{"type":"ping"}"#,
        ] {
            let event: WireStreamEvent = serde_json::from_str(raw).unwrap();
            assert!(
                matches!(event, WireStreamEvent::Other),
                "should ignore {raw}"
            );
        }
    }

    #[test]
    fn message_delta_carries_the_stop_reason() {
        let event: WireStreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":3}}"#,
        )
        .unwrap();
        assert!(matches!(
            event,
            WireStreamEvent::MessageDelta {
                delta: MessageDelta { stop_reason: Some(reason) }
            } if reason == "max_tokens"
        ));
    }

    #[test]
    fn error_event_carries_kind_and_message() {
        let event: WireStreamEvent = serde_json::from_str(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        )
        .unwrap();
        match event {
            WireStreamEvent::Error { error } => {
                assert_eq!(error.kind, "overloaded_error");
                assert_eq!(error.message, "Overloaded");
            }
            other => panic!("expected an error event, got {other:?}"),
        }
    }
}
