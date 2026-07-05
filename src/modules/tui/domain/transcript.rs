use std::time::Duration;

/// A rendered item in the conversation transcript. This is a render-only projection of the engine's
/// `Conversation`, which remains the single source of truth; the runtime never reads items back into
/// the engine. Streaming reasoning/content are coalesced in place so a turn does not produce one item
/// per delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    User(String),
    Reasoning(String),
    Assistant(String),
    PlanProposed(String),
    /// A tool call the agent made — its command, an optional edit diff, and (once it finishes) the
    /// outcome. Surfaced in every approval mode so the user sees each action even under auto.
    Tool(ToolActivity),
    Notice(NoticeLevel, String),
}

/// The severity of an out-of-band notice line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Error,
}

/// The terminal state of a tool call, for transcript coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Ok,
    Error,
    Declined,
}

/// The before/after text of an `edit_file` call, for an inline red/green diff. Derived from the call
/// arguments (`old_string`/`new_string`) in the adapter, so the tool itself stays untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDiff {
    pub old: String,
    pub new: String,
}

/// One tool call rendered in the transcript: the command, an optional edit diff, and the result once
/// it completes. `result` is `None` while the call is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivity {
    pub command: String,
    pub diff: Option<ToolDiff>,
    /// `(status, output, elapsed)` once finished; `None` while running. `output` is the full (capped)
    /// tool output the renderer previews or expands.
    pub result: Option<(ToolStatus, String, Duration)>,
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

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether the trailing item is an assistant answer — i.e. a content delta would coalesce into it
    /// rather than start a fresh one. Lets the reducer keep the streaming line-landing buffer aligned
    /// with the current answer.
    pub fn last_is_assistant(&self) -> bool {
        matches!(self.items.last(), Some(TranscriptItem::Assistant(_)))
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

    /// Push a running tool item (command + optional edit diff, no result yet). The engine runs one
    /// call at a time, so the running tool is always the trailing item — no correlation id is needed.
    pub fn push_tool_start(&mut self, command: String, diff: Option<ToolDiff>) {
        self.items.push(TranscriptItem::Tool(ToolActivity {
            command,
            diff,
            result: None,
        }));
    }

    /// Fill the trailing running tool item with its outcome. A no-op if the trailing item is not a
    /// running tool (defensive — e.g. an aborted turn left nothing to finish).
    pub fn finish_last_tool(&mut self, status: ToolStatus, output: String, elapsed: Duration) {
        if let Some(TranscriptItem::Tool(activity)) = self.items.last_mut()
            && activity.result.is_none()
        {
            activity.result = Some((status, output, elapsed));
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

    #[test]
    fn tool_start_then_finish_coalesce_into_one_item() {
        let mut t = Transcript::default();
        t.push_tool_start("edit a.txt".to_string(), None);
        assert_eq!(t.items().len(), 1);
        t.finish_last_tool(
            ToolStatus::Ok,
            "edited a.txt".to_string(),
            Duration::from_millis(5),
        );
        assert_eq!(
            t.items(),
            &[TranscriptItem::Tool(ToolActivity {
                command: "edit a.txt".to_string(),
                diff: None,
                result: Some((
                    ToolStatus::Ok,
                    "edited a.txt".to_string(),
                    Duration::from_millis(5)
                )),
            })]
        );
    }

    #[test]
    fn finish_without_a_running_tool_is_a_noop() {
        let mut t = Transcript::default();
        t.finish_last_tool(ToolStatus::Ok, "x".to_string(), Duration::ZERO);
        assert!(t.is_empty());
    }

    #[test]
    fn a_delta_after_a_tool_starts_a_fresh_assistant_item() {
        let mut t = Transcript::default();
        t.push_tool_start("cat a.txt".to_string(), None);
        t.push_content_delta("hi");
        assert_eq!(t.items().len(), 2);
        assert!(matches!(t.items()[1], TranscriptItem::Assistant(_)));
    }
}
