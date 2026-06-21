use serde::{Deserialize, Deserializer, Serialize};

use crate::models::tools::{Tool, ToolCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls requested by an assistant turn. Omitted on every other message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Set only on `Role::Tool`: which assistant tool call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// An assistant turn that requests tool calls. `content` is the optional narration the model
    /// emitted alongside the calls.
    pub fn assistant_tool_calls(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<ChatTemplateKwargs>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
}

/// Provider-specific knob that asks the model to emit reasoning. Reasoning models stream it by
/// default; sending this makes the intent explicit.
#[derive(Debug, Serialize)]
pub struct ChatTemplateKwargs {
    pub thinking: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChatStreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: Delta,
}

#[derive(Debug, Deserialize)]
pub struct Delta {
    pub content: Option<String>,
    /// Reasoning text under the standard `reasoning_content` name (vLLM/NVIDIA convention).
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning_content: Option<String>,
    /// Some providers (and NVIDIA Nemotron) also/instead send `reasoning`. Kept as its own field:
    /// a serde `alias` would make a delta carrying BOTH keys fail as a duplicate field.
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning: Option<String>,
    /// Tool-call fragments. Streamed incrementally and keyed by `index`; assembled by the service.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallFragment>,
}

/// One streamed slice of a tool call. Every field but `index` is optional: the first fragment for an
/// index carries `id`/`type`/`function.name`, later fragments carry only `function.arguments` slices.
#[derive(Debug, Deserialize)]
pub struct ToolCallFragment {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub function: Option<FunctionFragment>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionFragment {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Serde adapter: accept a string into `Some`; coerce any other JSON shape (object, list, number,
/// null) to `None`. Keeps an unexpected reasoning shape from failing the whole delta and dropping
/// its `content`.
fn string_or_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::String(text)) => Some(text),
            _ => None,
        },
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::tools::{FunctionCall, FunctionDef, ToolKind};

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
    }

    #[test]
    fn chat_request_serializes_expected_shape() {
        dotenvy::dotenv().ok();
        let model = std::env::var("NVIDIA_MODEL").expect("NVIDIA_MODEL must be set in .env");

        let request = ChatRequest {
            model: model.clone(),
            messages: vec![Message::user("hi")],
            stream: true,
            chat_template_kwargs: None,
            tools: Vec::new(),
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
            model: "m".to_string(),
            messages: vec![],
            stream: true,
            chat_template_kwargs: None,
            tools: Vec::new(),
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert!(value.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn chat_template_kwargs_serializes_nested() {
        let request = ChatRequest {
            model: "m".to_string(),
            messages: vec![],
            stream: true,
            chat_template_kwargs: Some(ChatTemplateKwargs { thinking: true }),
            tools: Vec::new(),
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["chat_template_kwargs"]["thinking"], true);
    }

    #[test]
    fn message_round_trips() {
        let message = Message::assistant_text("ok");
        let json = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::Assistant);
        assert_eq!(back.content.as_deref(), Some("ok"));
    }

    #[test]
    fn message_system_serializes_role_and_content() {
        let message = Message::system("be concise");
        assert_eq!(message.role, Role::System);
        assert_eq!(message.content.as_deref(), Some("be concise"));
        let value: serde_json::Value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["role"], "system");
        assert_eq!(value["content"], "be concise");
        assert!(value.get("tool_calls").is_none());
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
    fn assistant_tool_calls_message_omits_content_and_includes_tool_calls() {
        let message = Message::assistant_tool_calls(
            None,
            vec![ToolCall {
                id: "c1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"a.txt"}"#.to_string(),
                },
            }],
        );
        let value: serde_json::Value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["role"], "assistant");
        assert!(value.get("content").is_none());
        assert_eq!(value["tool_calls"][0]["id"], "c1");
        assert_eq!(value["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_result_message_serializes_role_and_tool_call_id() {
        let message = Message::tool_result("c1", "ok");
        let value: serde_json::Value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["role"], "tool");
        assert_eq!(value["tool_call_id"], "c1");
        assert_eq!(value["content"], "ok");
        assert!(value.get("tool_calls").is_none());
    }

    #[test]
    fn chat_request_omits_tools_when_empty() {
        let request = ChatRequest {
            model: "m".to_string(),
            messages: vec![],
            stream: true,
            chat_template_kwargs: None,
            tools: Vec::new(),
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert!(value.get("tools").is_none());
    }

    #[test]
    fn chat_request_includes_tools_when_present() {
        let request = ChatRequest {
            model: "m".to_string(),
            messages: vec![],
            stream: true,
            chat_template_kwargs: None,
            tools: vec![Tool {
                kind: ToolKind::Function,
                function: FunctionDef {
                    name: "read_file".to_string(),
                    description: "d".to_string(),
                    parameters: serde_json::json!({"type": "object"}),
                },
            }],
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
