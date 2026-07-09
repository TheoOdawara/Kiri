use serde::{Deserialize, Serialize};

use crate::shared::kernel::message::{Message, ThinkingBlock};
use crate::shared::kernel::role::Role;
use crate::shared::kernel::tool_call::ToolCall;

/// Serde mirror of the kernel `Message`, so the domain stays serde-free (ADR 0003).
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
    #[serde(default)]
    pub thinking: Option<ThinkingBlock>,
}

impl From<&Message> for StoredMessage {
    fn from(message: &Message) -> Self {
        Self {
            role: message.role.as_wire_str().to_string(),
            content: message.content.clone(),
            images: message.images.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            thinking: message.thinking.clone(),
        }
    }
}

impl StoredMessage {
    /// `None` on an unknown role, so the loader skips a corrupt row rather than fabricating one.
    pub fn into_domain(self) -> Option<Message> {
        let role = Role::from_wire_str(&self.role)?;
        Some(Message {
            role,
            content: self.content,
            images: self.images,
            tool_calls: self.tool_calls,
            tool_call_id: self.tool_call_id,
            thinking: self.thinking,
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
            thinking: None,
        };
        assert!(dto.into_domain().is_none());
    }

    #[test]
    fn round_trips_a_visible_thinking_block() {
        let message = Message::assistant_text("done").with_thinking(ThinkingBlock::Visible {
            text: "reasoning".to_string(),
            signature: Some("sig".to_string()),
        });
        let dto = StoredMessage::from(&message);
        let json = serde_json::to_string(&dto).unwrap();
        let back: StoredMessage = serde_json::from_str(&json).unwrap();
        let restored = back.into_domain().unwrap();
        match restored
            .thinking
            .expect("thinking must survive the round trip")
        {
            ThinkingBlock::Visible { text, signature } => {
                assert_eq!(text, "reasoning");
                assert_eq!(signature.as_deref(), Some("sig"));
            }
            other => panic!("expected Visible, got {other:?}"),
        }
    }

    #[test]
    fn round_trips_a_redacted_thinking_block() {
        let message = Message::assistant_text("done").with_thinking(ThinkingBlock::Redacted {
            data: "encrypted-blob".to_string(),
        });
        let restored = StoredMessage::from(&message).into_domain().unwrap();
        match restored
            .thinking
            .expect("thinking must survive the round trip")
        {
            ThinkingBlock::Redacted { data } => assert_eq!(data, "encrypted-blob"),
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn no_thinking_stays_none_across_the_round_trip() {
        let message = Message::assistant_text("done");
        let restored = StoredMessage::from(&message).into_domain().unwrap();
        assert!(restored.thinking.is_none());
    }
}
