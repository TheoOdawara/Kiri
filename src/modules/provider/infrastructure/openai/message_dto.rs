use serde::Serialize;

use crate::modules::agent::domain::message::Message;
use crate::modules::agent::domain::role::Role;
use crate::shared::kernel::tool_call::ToolCall;

/// The OpenAI-compatible wire shape of a chat message, built from a domain `Message`. The provider's
/// serialization rules (omit empty content / tool_calls / tool_call_id) live here, keeping the domain
/// `Message` free of any wire concern — so a future provider with a different message shape only adds
/// its own DTO.
#[derive(Debug, Serialize)]
pub struct MessageDto {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl From<&Message> for MessageDto {
    fn from(message: &Message) -> Self {
        Self {
            role: message.role,
            content: message.content.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::tool_call::FunctionCall;

    #[test]
    fn system_message_serializes_role_and_content() {
        let value = serde_json::to_value(MessageDto::from(&Message::system("be concise"))).unwrap();
        assert_eq!(value["role"], "system");
        assert_eq!(value["content"], "be concise");
        assert!(value.get("tool_calls").is_none());
    }

    #[test]
    fn assistant_text_serializes_content() {
        let value = serde_json::to_value(MessageDto::from(&Message::assistant_text("ok"))).unwrap();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"], "ok");
    }

    #[test]
    fn assistant_tool_calls_omits_content_and_includes_tool_calls() {
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
        let value = serde_json::to_value(MessageDto::from(&message)).unwrap();
        assert_eq!(value["role"], "assistant");
        assert!(value.get("content").is_none());
        assert_eq!(value["tool_calls"][0]["id"], "c1");
        assert_eq!(value["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_result_serializes_role_and_tool_call_id() {
        let value =
            serde_json::to_value(MessageDto::from(&Message::tool_result("c1", "ok"))).unwrap();
        assert_eq!(value["role"], "tool");
        assert_eq!(value["tool_call_id"], "c1");
        assert_eq!(value["content"], "ok");
        assert!(value.get("tool_calls").is_none());
    }
}
