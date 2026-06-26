use crate::modules::agent::domain::message::Message;

/// A persisted conversation: the ordered messages of one chat, scoped to a project. Reuses the agent
/// domain's `Message` directly (the session context depends on `agent`, one-directionally); the system
/// message is never stored — it is regenerated per session from the current memory digest.
pub struct Session {
    pub id: String,
    /// The workspace this session belongs to (`project_id_from_path`), so sessions list per project.
    /// Part of the loaded entity; read by the future sync/memory tooling, not by the resume path.
    #[allow(dead_code)]
    pub project_id: String,
    /// A short human label derived from the first user message; shown in the `/sessions` picker.
    pub title: String,
    /// Persisted timestamps, part of the loaded entity; reserved for the planned session-management UI.
    #[allow(dead_code)]
    pub created_at: String,
    #[allow(dead_code)]
    pub updated_at: String,
    pub messages: Vec<Message>,
}

/// A lightweight view of a session for the `/resume` and `/sessions` listings — no message bodies.
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub message_count: usize,
}

/// Derive a session title from the first user message: the first non-empty line, trimmed to a readable
/// length. Falls back to a generic label when there is no usable text (e.g. an image-only first turn).
pub fn derive_title(first_user_message: &str) -> String {
    const MAX: usize = 60;
    let line = first_user_message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.is_empty() {
        return "(sem título)".to_string();
    }
    let mut title: String = line.chars().take(MAX).collect();
    if line.chars().count() > MAX {
        title.push('…');
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_title_takes_the_first_nonempty_line() {
        assert_eq!(derive_title("\n  hello world \nsecond"), "hello world");
    }

    #[test]
    fn derive_title_truncates_long_input() {
        let long = "a".repeat(100);
        let title = derive_title(&long);
        assert_eq!(title.chars().count(), 61); // 60 chars + ellipsis
        assert!(title.ends_with('…'));
    }

    #[test]
    fn derive_title_falls_back_when_blank() {
        assert_eq!(derive_title("   \n  "), "(sem título)");
    }
}
