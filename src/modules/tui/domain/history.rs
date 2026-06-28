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

#[cfg(test)]
mod tests {
    use super::*;

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
