use serde::{Deserialize, Serialize};

use super::role::Role;
use crate::shared::kernel::tool_call::ToolCall;

/// An Anthropic extended-thinking block carried by an assistant turn, which the Messages API requires to
/// be replayed byte-for-byte ahead of any `tool_use` block on a later turn. Two genuinely different shapes,
/// not two states of one shape: `Visible` is the reasoning text (possibly empty, per `display: "omitted"`)
/// plus its verification `signature`; `Redacted` is an opaque encrypted blob the safety system substitutes
/// in its place (no readable text at all). Serde-derived (like `ToolCall`) so it round-trips through
/// session persistence verbatim (see `tool_call` and ADR 0003).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingBlock {
    Visible {
        text: String,
        signature: Option<String>,
    },
    Redacted {
        data: String,
    },
}

/// A single message in the conversation. Pure domain: no wire/serde concern of its own — the provider
/// maps it to its request shape via a DTO (see `provider::infrastructure::openai::message_dto`). Its
/// embedded exceptions are `ToolCall` and `ThinkingBlock`, both serde-derived because they are persisted
/// verbatim for session history (see `tool_call` and ADR 0003).
#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    /// Image data URLs attached to a user message (base64 PNG, `data:image/png;base64,...`). Empty on
    /// every other message; when non-empty the provider emits multimodal `content` parts.
    pub images: Vec<String>,
    /// Tool calls requested by an assistant turn. Empty on every other message.
    pub tool_calls: Vec<ToolCall>,
    /// Set only on `Role::Tool`: which assistant tool call this message answers.
    pub tool_call_id: Option<String>,
    /// An Anthropic extended-thinking block carried by an assistant turn. `None` on every other
    /// provider/message. Set via [`Message::with_thinking`], never a constructor argument, so the many
    /// existing `Message` call sites are unaffected.
    pub thinking: Option<ThinkingBlock>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            thinking: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            thinking: None,
        }
    }

    /// A user message carrying images alongside its text. `images` are `image_url` data URLs; the
    /// provider serializes the message as multimodal `content` parts (text part + one part per image).
    pub fn user_multimodal(text: impl Into<String>, images: Vec<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            images,
            tool_calls: Vec::new(),
            tool_call_id: None,
            thinking: None,
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(text.into()),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            thinking: None,
        }
    }

    /// An assistant turn that requests tool calls. `content` is the optional narration the model
    /// emitted alongside the calls.
    pub fn assistant_tool_calls(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            images: Vec::new(),
            tool_calls,
            tool_call_id: None,
            thinking: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            thinking: None,
        }
    }

    /// Attach an extended-thinking block to an assistant turn (see [`ThinkingBlock`]). The only mutator
    /// for `thinking`, so every other constructor stays untouched by its addition.
    pub fn with_thinking(mut self, thinking: ThinkingBlock) -> Self {
        self.thinking = Some(thinking);
        self
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

    #[test]
    fn with_thinking_attaches_a_block_and_defaults_to_none() {
        let plain = Message::assistant_text("done");
        assert!(plain.thinking.is_none());

        let reasoned = Message::assistant_text("done").with_thinking(ThinkingBlock::Visible {
            text: "reasoning".to_string(),
            signature: Some("sig".to_string()),
        });
        match reasoned.thinking.expect("thinking must be attached") {
            ThinkingBlock::Visible { text, signature } => {
                assert_eq!(text, "reasoning");
                assert_eq!(signature.as_deref(), Some("sig"));
            }
            other => panic!("expected Visible, got {other:?}"),
        }
    }

    #[test]
    fn with_thinking_attaches_a_redacted_block() {
        let reasoned = Message::assistant_text("done").with_thinking(ThinkingBlock::Redacted {
            data: "encrypted-blob".to_string(),
        });
        match reasoned.thinking.expect("thinking must be attached") {
            ThinkingBlock::Redacted { data } => assert_eq!(data, "encrypted-blob"),
            other => panic!("expected Redacted, got {other:?}"),
        }
    }
}
