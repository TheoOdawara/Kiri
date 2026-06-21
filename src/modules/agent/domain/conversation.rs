use super::message::Message;
use super::role::Role;

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
}
