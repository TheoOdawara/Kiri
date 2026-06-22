/// The multi-line input editor buffer: the text plus the cursor as a byte offset that always sits on a
/// char boundary. Hand-rolled (no widget crate) — the needs are modest and native-over-deps applies.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InputBuffer {
    text: String,
    cursor: usize,
}

impl InputBuffer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if let Some(prev) = self.text[..self.cursor].chars().next_back() {
            let start = self.cursor - prev.len_utf8();
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    pub fn delete(&mut self) {
        if let Some(next) = self.text[self.cursor..].chars().next() {
            let end = self.cursor + next.len_utf8();
            self.text.replace_range(self.cursor..end, "");
        }
    }

    pub fn left(&mut self) {
        if let Some(prev) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(next) = self.text[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }

    pub fn end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len());
    }

    /// Replace the whole buffer (history recall), placing the cursor at the end.
    pub fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }

    /// Take the text out, leaving the buffer empty.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn line_count(&self) -> usize {
        self.text.bytes().filter(|&b| b == b'\n').count() + 1
    }
}

/// Submitted-prompt history with shell-style up/down recall. The in-progress line is saved as a draft
/// when navigation starts and restored when navigating past the newest entry.
#[derive(Debug, Default, Clone)]
pub struct History {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: String,
}

impl History {
    /// Record a submitted line (trimmed, non-empty, de-duplicated against the last) and reset navigation.
    pub fn record(&mut self, line: &str) {
        self.cursor = None;
        self.draft.clear();
        let line = line.trim();
        if line.is_empty() || self.entries.last().is_some_and(|last| last == line) {
            return;
        }
        self.entries.push(line.to_string());
    }

    /// Step to an older entry, saving `current` as the draft on the first step.
    pub fn older(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        match self.cursor {
            None => {
                self.draft = current.to_string();
                self.cursor = Some(self.entries.len() - 1);
            }
            Some(0) => {}
            Some(i) => self.cursor = Some(i - 1),
        }
        self.cursor.map(|i| self.entries[i].clone())
    }

    /// Step to a newer entry; past the newest, return the saved draft.
    pub fn newer(&mut self) -> Option<String> {
        match self.cursor {
            None => None,
            Some(i) if i + 1 < self.entries.len() => {
                self.cursor = Some(i + 1);
                Some(self.entries[i + 1].clone())
            }
            Some(_) => {
                self.cursor = None;
                Some(std::mem::take(&mut self.draft))
            }
        }
    }
}

/// Transcript scroll position, measured as lines scrolled up from the newest content. Zero means
/// pinned to the bottom (auto-following new output). The view clamps it to the available scrollback.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    pub scrollback: u16,
}

impl Scroll {
    pub fn up(&mut self, lines: u16) {
        self.scrollback = self.scrollback.saturating_add(lines);
    }

    pub fn down(&mut self, lines: u16) {
        self.scrollback = self.scrollback.saturating_sub(lines);
    }

    pub fn pin(&mut self) {
        self.scrollback = 0;
    }
}

/// A tool-call (or runaway-checkpoint) confirmation awaiting the user's answer. Pure data — the reply
/// channel lives in the runtime, since the engine handles approvals one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub prompt: String,
    pub default_accept: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_edits_respect_char_boundaries() {
        let mut b = InputBuffer::default();
        b.insert("ação");
        b.left(); // cursor now sits before the final 'o'
        b.backspace(); // removes the multibyte 'ã' just before the cursor
        assert_eq!(b.text(), "aço");
        assert_eq!(b.cursor(), "aç".len());
    }

    #[test]
    fn home_and_end_move_within_the_current_line() {
        let mut b = InputBuffer::default();
        b.insert("ab\ncd");
        b.home();
        assert_eq!(b.cursor(), 3);
        b.end();
        assert_eq!(b.cursor(), 5);
    }

    #[test]
    fn history_recalls_older_then_restores_draft() {
        let mut h = History::default();
        h.record("first");
        h.record("second");
        assert_eq!(h.older("draft").as_deref(), Some("second"));
        assert_eq!(h.older("draft").as_deref(), Some("first"));
        assert_eq!(h.newer().as_deref(), Some("second"));
        assert_eq!(h.newer().as_deref(), Some("draft"));
    }

    #[test]
    fn history_skips_consecutive_duplicates() {
        let mut h = History::default();
        h.record("x");
        h.record("x");
        assert_eq!(h.older("").as_deref(), Some("x"));
        assert_eq!(h.older("").as_deref(), Some("x"));
    }
}
