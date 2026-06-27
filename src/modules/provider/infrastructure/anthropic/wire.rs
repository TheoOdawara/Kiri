//! Pure wire DTOs for the Anthropic Messages API: the request body (serialize) and the streamed event
//! payloads (deserialize). The domain→wire message/tool translation lives in `message_dto`; the
//! stream assembly in `sse`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message_dto::AnthropicMessage;

/// The Anthropic Messages API request body. `system` is a top-level field (not a message); `messages`
/// are the alternating user/assistant turns built by `message_dto`; `tools` are the Anthropic-shaped
/// schemas translated from the registry's OpenAI shape. No `thinking`/`output_config` is sent — see the
/// extended-thinking note on `AnthropicProvider`.
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
}

/// One streamed Server-Sent Event from the Messages API, dispatched on its `type` discriminator. Only
/// the events that carry assembly-relevant data are modeled; everything else (`message_start`,
/// `content_block_stop`, `message_delta`, `message_stop`, `ping`, future types) falls into `Other` and
/// is ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamEventDto {
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockStartDto,
    },
    ContentBlockDelta {
        index: u32,
        delta: BlockDeltaDto,
    },
    /// The end-of-message delta, carrying `stop_reason` (`"max_tokens"` means the output cap truncated
    /// the turn). Modeled so a silent truncation can be surfaced.
    MessageDelta {
        delta: MessageDeltaDto,
    },
    /// An in-stream error the API can deliver on an otherwise-200 SSE response.
    Error {
        error: ApiErrorDto,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaDto {
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// The opening descriptor of a content block. Only `tool_use` carries data we need (its id + name);
/// text/thinking blocks fall into `Other`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlockStartDto {
    ToolUse {
        id: String,
        name: String,
    },
    #[serde(other)]
    Other,
}

/// An incremental update to a content block. `text_delta` is answer content, `thinking_delta` is
/// reasoning, `input_json_delta` is a slice of a tool call's JSON input; other delta kinds
/// (`signature_delta`, …) are ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum BlockDeltaDto {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiErrorDto {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_block_start_reads_tool_use_id_and_name() {
        let event: StreamEventDto = serde_json::from_str(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}"#,
        )
        .unwrap();
        match event {
            StreamEventDto::ContentBlockStart {
                index,
                content_block: ContentBlockStartDto::ToolUse { id, name },
            } => {
                assert_eq!(index, 1);
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "read_file");
            }
            other => panic!("expected a tool_use start, got {other:?}"),
        }
    }

    #[test]
    fn text_block_start_falls_into_other() {
        let event: StreamEventDto = serde_json::from_str(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        )
        .unwrap();
        assert!(matches!(
            event,
            StreamEventDto::ContentBlockStart {
                content_block: ContentBlockStartDto::Other,
                ..
            }
        ));
    }

    #[test]
    fn deltas_deserialize_by_kind() {
        let text: StreamEventDto = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
        )
        .unwrap();
        assert!(matches!(
            text,
            StreamEventDto::ContentBlockDelta {
                delta: BlockDeltaDto::TextDelta { text },
                ..
            } if text == "Hi"
        ));

        let json: StreamEventDto = serde_json::from_str(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"p\""}}"#,
        )
        .unwrap();
        assert!(matches!(
            json,
            StreamEventDto::ContentBlockDelta {
                delta: BlockDeltaDto::InputJsonDelta { partial_json },
                ..
            } if partial_json == "{\"p\""
        ));
    }

    #[test]
    fn message_lifecycle_events_are_ignored() {
        for raw in [
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
            r#"{"type":"ping"}"#,
        ] {
            let event: StreamEventDto = serde_json::from_str(raw).unwrap();
            assert!(
                matches!(event, StreamEventDto::Other),
                "should ignore {raw}"
            );
        }
    }

    #[test]
    fn message_delta_carries_the_stop_reason() {
        let event: StreamEventDto = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":3}}"#,
        )
        .unwrap();
        assert!(matches!(
            event,
            StreamEventDto::MessageDelta {
                delta: MessageDeltaDto { stop_reason: Some(reason) }
            } if reason == "max_tokens"
        ));
    }

    #[test]
    fn error_event_carries_kind_and_message() {
        let event: StreamEventDto = serde_json::from_str(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        )
        .unwrap();
        match event {
            StreamEventDto::Error { error } => {
                assert_eq!(error.kind, "overloaded_error");
                assert_eq!(error.message, "Overloaded");
            }
            other => panic!("expected an error event, got {other:?}"),
        }
    }
}
