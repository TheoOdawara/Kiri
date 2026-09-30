use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::message_dto::WireMessage;

#[derive(Debug, Serialize)]
pub(crate) struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<WireMessage<'a>>,
    pub stream: bool,
    /// NVIDIA Nemotron-style reasoning toggle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<ChatTemplateKwargs>,
    /// OpenAI proper (o3/o4) reasoning effort: `"low"`, `"medium"`, or `"high"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub tools: &'a [Value],
}

/// NVIDIA hosts two confirmed keys for the same concept: `thinking` (Nemotron, Kimi) and
/// `enable_thinking` (Qwen, GLM). Exactly one is ever populated per family; the rest send neither.
#[derive(Debug, Serialize)]
pub(crate) struct ChatTemplateKwargs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatStreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    /// A provider can deliver an error in-band on an HTTP 200 stream. Captured on the chunk so the hot
    /// path parses each delta once, not twice.
    #[serde(default)]
    pub error: Option<StreamError>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamError {
    #[serde(default)]
    pub message: Option<String>,
    /// A raw `Value`: providers send this as either a string or a number.
    #[serde(default)]
    pub code: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChoice {
    pub delta: Delta,
    /// `"length"` means the output token cap truncated the turn, which must not pass as a silent stop.
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Delta {
    pub content: Option<String>,
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning_content: Option<String>,
    /// A separate field, not a serde `alias`: a delta carrying BOTH keys would fail as a duplicate.
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallFragment>,
}

/// The first fragment for an `index` carries `id`/`type`/`function.name`; later ones carry only
/// `function.arguments` slices.
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

/// Coerces any non-string shape to `None`, so an unexpected reasoning payload cannot fail the whole
/// delta and take its `content` down with it.
fn string_or_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(deserializer)? {
        Some(Value::String(text)) => Some(text),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::{ChatRequest, ChatTemplateKwargs, Delta, ToolCallFragment};
    use crate::modules::provider::infrastructure::openai::message_dto::WireMessage;
    use crate::shared::kernel::message::Message;

    #[test]
    fn chat_request_serializes_expected_shape() {
        let model = "test-model";

        let message = Message::user("hi");
        let request = ChatRequest {
            model,
            messages: vec![WireMessage::from(&message)],
            stream: true,
            chat_template_kwargs: None,
            reasoning_effort: None,
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
            reasoning_effort: None,
            tools: &[],
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert!(value.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn chat_template_kwargs_serializes_the_thinking_key() {
        let request = ChatRequest {
            model: "m",
            messages: vec![],
            stream: true,
            chat_template_kwargs: Some(ChatTemplateKwargs {
                thinking: Some(true),
                enable_thinking: None,
            }),
            reasoning_effort: None,
            tools: &[],
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["chat_template_kwargs"]["thinking"], true);
        assert!(
            value["chat_template_kwargs"]
                .get("enable_thinking")
                .is_none()
        );
    }

    #[test]
    fn chat_template_kwargs_serializes_the_enable_thinking_key() {
        let request = ChatRequest {
            model: "m",
            messages: vec![],
            stream: true,
            chat_template_kwargs: Some(ChatTemplateKwargs {
                thinking: None,
                enable_thinking: Some(true),
            }),
            reasoning_effort: None,
            tools: &[],
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["chat_template_kwargs"]["enable_thinking"], true);
        assert!(value["chat_template_kwargs"].get("thinking").is_none());
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
        // NVIDIA Nemotron streams both keys together.
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
            reasoning_effort: None,
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
            reasoning_effort: None,
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
