//! The conversation cluster: the shared data the `agent` engine and the `provider` adapters exchange.
//! `conversation.rs` owns the `Conversation` type and groups its siblings (`message`, `role`,
//! `completed_turn`, `stream_event`) under `conversation/`; the kernel root re-exports them so the
//! pre-grouping `shared::kernel::{message,role,…}` paths still resolve.

pub mod completed_turn;
pub mod message;
pub mod role;
pub mod stream_event;

use self::message::Message;
use self::role::Role;

/// The running conversation: the system seed plus every user/assistant/tool message, in order.
pub struct Conversation {
    messages: Vec<Message>,
}

impl Conversation {
    pub fn new(system_prompt: impl Into<String>) -> Self {
        Self {
            messages: vec![Message::system(system_prompt)],
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Drop a trailing user message left dangling by a failed first round; a partial tool exchange
    /// (the last message is a tool result) is a resumable state and is kept.
    pub fn rollback_dangling_user(&mut self) {
        if matches!(self.messages.last(), Some(message) if message.role == Role::User) {
            self.messages.pop();
        }
    }

    /// Drop the last assistant turn — the assistant message and any tool results that answered it —
    /// so the conversation can recover after the provider rejected the request body (HTTP 4xx). That
    /// offending turn would otherwise be re-sent unchanged and fail identically on every later
    /// request. A no-op when no assistant turn trails the conversation.
    pub fn rollback_last_assistant_turn(&mut self) {
        while matches!(self.messages.last(), Some(message) if message.role == Role::Tool) {
            self.messages.pop();
        }
        if matches!(self.messages.last(), Some(message) if message.role == Role::Assistant) {
            self.messages.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_pops_only_a_dangling_user() {
        let mut conversation = Conversation::new("sys");
        conversation.push(Message::user("hi"));
        conversation.rollback_dangling_user();
        assert_eq!(conversation.messages().len(), 1); // system only

        conversation.push(Message::user("again"));
        conversation.push(Message::tool_result("id", "out"));
        conversation.rollback_dangling_user();
        assert_eq!(conversation.messages().len(), 3); // system, user, tool — partial exchange kept
    }

    fn roles(conversation: &Conversation) -> Vec<Role> {
        conversation.messages().iter().map(|m| m.role).collect()
    }

    #[test]
    fn rollback_last_assistant_turn_drops_the_assistant_and_its_tool_results() {
        let mut conversation = Conversation::new("sys");
        conversation.push(Message::user("do it"));
        conversation.push(Message::assistant_tool_calls(None, vec![]));
        conversation.push(Message::tool_result("id1", "ok"));
        conversation.push(Message::tool_result("id2", "ok"));
        conversation.rollback_last_assistant_turn();
        assert_eq!(roles(&conversation), vec![Role::System, Role::User]);
    }

    #[test]
    fn rollback_last_assistant_turn_is_a_noop_without_an_assistant_turn() {
        let mut conversation = Conversation::new("sys");
        conversation.push(Message::user("hi"));
        conversation.rollback_last_assistant_turn();
        assert_eq!(roles(&conversation), vec![Role::System, Role::User]);
    }

    #[test]
    fn rollback_last_assistant_turn_keeps_earlier_exchanges() {
        let mut conversation = Conversation::new("sys");
        conversation.push(Message::user("first"));
        conversation.push(Message::assistant_text("answer"));
        conversation.push(Message::user("second"));
        conversation.push(Message::assistant_tool_calls(None, vec![]));
        conversation.push(Message::tool_result("id", "ok"));
        conversation.rollback_last_assistant_turn();
        assert_eq!(
            roles(&conversation),
            vec![Role::System, Role::User, Role::Assistant, Role::User]
        );
    }
}
