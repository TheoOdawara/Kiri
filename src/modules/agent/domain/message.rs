use serde::{Deserialize, Serialize};

use super::role::Role;
use crate::shared::kernel::tool_call::ToolCall;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::tool_call::FunctionCall;

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
}
