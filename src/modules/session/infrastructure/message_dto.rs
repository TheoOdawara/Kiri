use serde::{Deserialize, Serialize};

use crate::modules::agent::domain::message::Message;
use crate::modules::agent::domain::role::Role;
use crate::shared::kernel::tool_call::ToolCall;

/// Serde mirror of the agent-domain `Message`, owned by this infrastructure layer so the domain stays
/// serde-free (ADR 0003). The `images` and `tool_calls` columns are stored as JSON; this type centralizes
/// that mapping for the SQLite session store.
#[derive(Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

/// The wire string for a role. Kept local to the store's serialization concern.
pub fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// Parse a stored role string. Unknown values map to `None` so a corrupted row can be skipped defensively
/// rather than panicking (the DB may have been touched by an external tool).
pub fn role_from_str(s: &str) -> Option<Role> {
    match s {
        "system" => Some(Role::System),
        "user" => Some(Role::User),
        "assistant" => Some(Role::Assistant),
        "tool" => Some(Role::Tool),
        _ => None,
    }
}

impl From<&Message> for StoredMessage {
    fn from(message: &Message) -> Self {
        Self {
            role: role_to_str(message.role).to_string(),
            content: message.content.clone(),
            images: message.images.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
        }
    }
}

impl StoredMessage {
    /// Reconstruct a domain `Message`, consuming the DTO. Returns `None` for an unknown role, so the
    /// loader can skip a corrupted row rather than fabricating a wrong one.
    pub fn into_domain(self) -> Option<Message> {
        let role = role_from_str(&self.role)?;
        Some(Message {
            role,
            content: self.content,
            images: self.images,
            tool_calls: self.tool_calls,
            tool_call_id: self.tool_call_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::tool_call::FunctionCall;

    #[test]
    fn round_trips_an_assistant_tool_call() {
        let message = Message::assistant_tool_calls(
            Some("narration".to_string()),
            vec![ToolCall {
                id: "c1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "write_file".to_string(),
                    arguments: r#"{"path":"a.txt"}"#.to_string(),
                },
            }],
        );
        let dto = StoredMessage::from(&message);
        let json = serde_json::to_string(&dto).unwrap();
        let back: StoredMessage = serde_json::from_str(&json).unwrap();
        let restored = back.into_domain().unwrap();
        assert_eq!(restored.role, Role::Assistant);
        assert_eq!(restored.content.as_deref(), Some("narration"));
        assert_eq!(restored.tool_calls.len(), 1);
        assert_eq!(restored.tool_calls[0].function.name, "write_file");
    }

    #[test]
    fn round_trips_a_tool_result() {
        let message = Message::tool_result("c1", "output");
        let restored = StoredMessage::from(&message).into_domain().unwrap();
        assert_eq!(restored.role, Role::Tool);
        assert_eq!(restored.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(restored.content.as_deref(), Some("output"));
    }

    #[test]
    fn unknown_role_is_skipped() {
        let dto = StoredMessage {
            role: "bogus".to_string(),
            content: None,
            images: vec![],
            tool_calls: vec![],
            tool_call_id: None,
        };
        assert!(dto.into_domain().is_none());
    }
}
