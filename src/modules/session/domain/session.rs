use crate::shared::kernel::message::Message;

/// The system message is never stored — it is regenerated per session from the memory digest.
pub struct Session {
    pub id: String,
    pub title: String,
    pub messages: Vec<Message>,
    /// Stored rows `load` dropped as corrupt; the resume path surfaces a Notice when non-zero.
    pub skipped_messages: usize,
}

pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub message_count: usize,
}

pub const UNTITLED_SESSION_LABEL: &str = "(sem título)";

pub fn derive_title(first_user_message: &str) -> String {
    const MAX: usize = 60;
    let line = first_user_message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.is_empty() {
        return UNTITLED_SESSION_LABEL.to_string();
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
    fn derive_title_uses_single_untitled_const() {
        assert_eq!(derive_title("   \n  "), UNTITLED_SESSION_LABEL);
    }
}
