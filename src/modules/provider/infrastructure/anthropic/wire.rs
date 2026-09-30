use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message_dto::AnthropicMessage;

#[derive(Debug, Serialize)]
pub(crate) struct MessagesRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub stream: bool,
    /// A top-level field, not a message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// Only valid alongside `thinking: {type: "adaptive"}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

/// `enabled` sets `display: "summarized"` explicitly, so the returned `thinking` text is never silently
/// empty on a model whose default differs. `disabled` must be sent, not merely omitted, on a model whose
/// adaptive thinking defaults on.
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

#[derive(Debug, Serialize)]
pub(crate) struct OutputConfig {
    pub effort: &'static str,
}

/// Only the assembly-relevant events are modeled; every other type falls into `Other` and is ignored.
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
    /// Carries `stop_reason`; `"max_tokens"` means the output cap truncated the turn.
    MessageDelta {
        delta: MessageDelta,
    },
    /// The API can deliver this on an otherwise-200 SSE response.
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

/// `redacted_thinking` arrives whole here: there is no delta kind for it, unlike `thinking`.
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

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum BlockDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    /// Must be replayed byte-for-byte ahead of any `tool_use` block on a later turn.
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
