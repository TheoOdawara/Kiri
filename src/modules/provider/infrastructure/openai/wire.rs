use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::message_dto::MessageDto;

/// The OpenAI-compatible chat-completions request body. A pure wire DTO: `messages` are mapped from
/// domain `Message`s through `MessageDto`, and `tools` are the opaque JSON schemas the tool registry
/// produced, passed through verbatim.
#[derive(Debug, Serialize)]
pub(crate) struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<MessageDto<'a>>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<ChatTemplateKwargs>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub tools: &'a [Value],
}

/// Provider-specific knob that asks the model to emit reasoning. Reasoning models stream it by
/// default; sending this makes the intent explicit.
#[derive(Debug, Serialize)]
pub(crate) struct ChatTemplateKwargs {
    pub thinking: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatStreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    /// An OpenAI-compatible provider can deliver an error in-band on an HTTP 200 stream
    /// (`data: {"error": {...}}`). Captured on the chunk so a single parse handles both the normal and
    /// error shapes — the streaming hot path no longer parses every token-delta twice.
    #[serde(default)]
    pub error: Option<StreamError>,
}

/// An in-band stream error. `code` is kept as a raw `Value` because providers send it as either a
/// string or a number.
#[derive(Debug, Deserialize)]
pub(crate) struct StreamError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub code: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChoice {
    pub delta: Delta,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Delta {
    pub content: Option<String>,
    /// Reasoning text under the standard `reasoning_content` name (vLLM/NVIDIA convention).
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning_content: Option<String>,
    /// Some providers (and NVIDIA Nemotron) also/instead send `reasoning`. Kept as its own field:
    /// a serde `alias` would make a delta carrying BOTH keys fail as a duplicate field.
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning: Option<String>,
    /// Tool-call fragments. Streamed incrementally and keyed by `index`; assembled by the SSE layer.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallFragment>,
}

/// One streamed slice of a tool call. Every field but `index` is optional: the first fragment for an
/// index carries `id`/`type`/`function.name`, later fragments carry only `function.arguments` slices.
#[derive(Debug, Deserialize)]
pub(crate) struct ToolCallFragment {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub function: Option<FunctionFragment>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FunctionFragment {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Serde adapter: accept a string into `Some`; coerce any other JSON shape (object, list, number,
/// null) to `None`. Keeps an unexpected reasoning shape from failing the whole delta and dropping
/// its `content`.
fn string_or_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(deserializer)? {
        Some(Value::String(text)) => Some(text),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::{ChatRequest, ChatTemplateKwargs, Delta, ToolCallFragment};
    use crate::modules::agent::domain::message::Message;
    use crate::modules::provider::infrastructure::openai::message_dto::MessageDto;

    #[test]
    fn chat_request_serializes_expected_shape() {
        let model = "test-model";

        let message = Message::user("hi");
        let request = ChatRequest {
            model,
            messages: vec![MessageDto::from(&message)],
            stream: true,
            chat_template_kwargs: None,
            tools: &[],
        };

        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["model"], model);
        assert_eq!(value["stream"], true);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"], "hi");
    }

    #[test]
    fn chat_template_kwargs_omitted_when_none() {
        let request = ChatRequest {
            model: "m",
            messages: vec![],
            stream: true,
            chat_template_kwargs: None,
            tools: &[],
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert!(value.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn chat_template_kwargs_serializes_nested() {
        let request = ChatRequest {
            model: "m",
            messages: vec![],
            stream: true,
            chat_template_kwargs: Some(ChatTemplateKwargs { thinking: true }),
            tools: &[],
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["chat_template_kwargs"]["thinking"], true);
    }

    #[test]
    fn delta_keeps_content_when_reasoning_is_not_a_string() {
        let delta: Delta =
            serde_json::from_str(r#"{"reasoning":{"step":1},"content":"Hi"}"#).unwrap();
        assert_eq!(delta.content.as_deref(), Some("Hi"));
        assert_eq!(delta.reasoning, None);
        assert_eq!(delta.reasoning_content, None);
    }

    #[test]
    fn delta_reads_reasoning_string() {
        let delta: Delta = serde_json::from_str(r#"{"reasoning_content":"why"}"#).unwrap();
        assert_eq!(delta.reasoning_content.as_deref(), Some("why"));
    }

    #[test]
    fn delta_accepts_both_reasoning_keys_at_once() {
        // NVIDIA Nemotron streams `reasoning` and `reasoning_content` together; this must not fail.
        let delta: Delta =
            serde_json::from_str(r#"{"reasoning":"Okay","reasoning_content":"Okay"}"#).unwrap();
        assert_eq!(delta.reasoning.as_deref(), Some("Okay"));
        assert_eq!(delta.reasoning_content.as_deref(), Some("Okay"));
    }

    #[test]
    fn chat_request_omits_tools_when_empty() {
        let request = ChatRequest {
            model: "m",
            messages: vec![],
            stream: true,
            chat_template_kwargs: None,
            tools: &[],
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert!(value.get("tools").is_none());
    }

    #[test]
    fn chat_request_includes_tools_when_present() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "d",
                "parameters": {"type": "object"}
            }
        })];
        let request = ChatRequest {
            model: "m",
            messages: vec![],
            stream: true,
            chat_template_kwargs: None,
            tools: &tools,
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(value["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_call_fragment_deserializes_partial() {
        let frag: ToolCallFragment =
            serde_json::from_str(r#"{"index":0,"function":{"arguments":"{\"p\""}}"#).unwrap();
        assert_eq!(frag.index, 0);
        assert_eq!(frag.id, None);
        assert_eq!(frag.kind, None);
        assert_eq!(frag.function.unwrap().arguments.as_deref(), Some("{\"p\""));
    }
}
