/// A rendered item in the conversation transcript. This is a render-only projection of the engine's
/// `Conversation`, which remains the single source of truth; the runtime never reads items back into
/// the engine. Streaming reasoning/content are coalesced in place so a turn does not produce one item
/// per delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    User(String),
    Reasoning(String),
    Assistant(String),
    Notice(NoticeLevel, String),
}

/// The severity of an out-of-band notice line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Error,
}

/// The ordered list of transcript items rendered in the main pane.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Transcript {
    items: Vec<TranscriptItem>,
}

impl Transcript {
    pub fn items(&self) -> &[TranscriptItem] {
        &self.items
    }

    pub fn push(&mut self, item: TranscriptItem) {
        self.items.push(item);
    }

    /// Append a streamed content delta, coalescing into a trailing assistant item.
    pub fn push_content_delta(&mut self, delta: &str) {
        match self.items.last_mut() {
            Some(TranscriptItem::Assistant(text)) => text.push_str(delta),
            _ => self
                .items
                .push(TranscriptItem::Assistant(delta.to_string())),
        }
    }

    /// Append a streamed reasoning delta, coalescing into a trailing reasoning item.
    pub fn push_reasoning_delta(&mut self, delta: &str) {
        match self.items.last_mut() {
            Some(TranscriptItem::Reasoning(text)) => text.push_str(delta),
            _ => self
                .items
                .push(TranscriptItem::Reasoning(delta.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_deltas_coalesce_into_one_assistant_item() {
        let mut t = Transcript::default();
        t.push_content_delta("Hel");
        t.push_content_delta("lo");
        assert_eq!(t.items(), &[TranscriptItem::Assistant("Hello".to_string())]);
    }

    #[test]
    fn reasoning_then_content_are_separate_items() {
        let mut t = Transcript::default();
        t.push_reasoning_delta("thinking");
        t.push_content_delta("answer");
        assert_eq!(
            t.items(),
            &[
                TranscriptItem::Reasoning("thinking".to_string()),
                TranscriptItem::Assistant("answer".to_string()),
            ]
        );
    }

    #[test]
    fn a_delta_after_a_notice_starts_a_fresh_item() {
        let mut t = Transcript::default();
        t.push(TranscriptItem::Notice(NoticeLevel::Info, "n".to_string()));
        t.push_content_delta("hi");
        assert_eq!(t.items().len(), 2);
    }
}
