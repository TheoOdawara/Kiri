use super::role::Role;
use crate::shared::kernel::tool_call::ToolCall;

/// A single message in the conversation. Pure domain: no wire/serde concern — the provider maps it to
/// its own request shape via a DTO (see `provider::infrastructure::openai::message_dto`).
#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    /// Tool calls requested by an assistant turn. Empty on every other message.
    pub tool_calls: Vec<ToolCall>,
    /// Set only on `Role::Tool`: which assistant tool call this message answers.
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

    #[test]
    fn constructors_carry_their_fields() {
        let user = Message::user("u");
        assert_eq!(user.content.as_deref(), Some("u"));
        assert!(user.tool_calls.is_empty());
        assert!(user.tool_call_id.is_none());

        let result = Message::tool_result("id1", "out");
        assert_eq!(result.content.as_deref(), Some("out"));
        assert_eq!(result.tool_call_id.as_deref(), Some("id1"));

        let narrated = Message::assistant_tool_calls(Some("n".to_string()), Vec::new());
        assert_eq!(narrated.content.as_deref(), Some("n"));
    }
}
