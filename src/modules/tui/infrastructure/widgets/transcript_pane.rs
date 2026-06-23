use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::infrastructure::markdown;
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::splash;

/// Render the scrolling transcript. Items are pre-wrapped to the pane width (so scroll offsets are
/// exact line counts), then scrolled so the newest content is pinned to the bottom unless the user has
/// scrolled up (`scrollback`). Assistant and reasoning items are parsed as markdown so bold, italics,
/// code spans, lists, and headings render with styled spans instead of raw `**`/`*`. When the
/// conversation is empty, the brand splash takes the pane instead.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    if model.transcript.is_empty() {
        splash::render(frame, area);
        return;
    }

    let width = area.width.max(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for item in model.transcript.items() {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        render_item(item, width, &mut lines);
    }

    let total = lines.len() as u16;
    let max_offset = total.saturating_sub(area.height);
    let scrollback = model.scroll.scrollback.min(max_offset);
    let offset = max_offset - scrollback;

    frame.render_widget(
        Paragraph::new(lines)
            .scroll((offset, 0))
            .style(theme::base()),
        area,
    );
}

fn render_item(item: &TranscriptItem, width: usize, out: &mut Vec<Line<'static>>) {
    match item {
        TranscriptItem::User(text) => push_wrapped(
            &format!("você › {text}"),
            width,
            Style::default()
                .fg(theme::HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
            out,
        ),
        TranscriptItem::Reasoning(text) => {
            let mut rendered = markdown::render(text, theme::dim(), width);
            out.append(&mut rendered);
        }
        TranscriptItem::Assistant(text) => {
            let mut rendered = markdown::render(text, Style::default().fg(theme::STEEL), width);
            out.append(&mut rendered);
        }
        TranscriptItem::Notice(level, text) => {
            let color = match level {
                NoticeLevel::Info => theme::WARNING,
                NoticeLevel::Error => theme::ERROR,
            };
            push_wrapped(text, width, Style::default().fg(color), out);
        }
    }
}

fn push_wrapped(text: &str, width: usize, style: Style, out: &mut Vec<Line<'static>>) {
    for row in hard_wrap(text, width) {
        out.push(Line::styled(row, style));
    }
}

/// Wrap by word to the pane width, preserving blank lines. Words longer than the width are hard-cut
/// so they never overflow. Width is a char count (display-width approximation), consistent with the
/// rest of the renderer and good enough for the transcript's mostly-ASCII content.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for logical in text.split('\n') {
        if logical.is_empty() {
            rows.push(String::new());
            continue;
        }
        wrap_line(logical, width, &mut rows);
    }
    rows
}

/// Greedy word-wrap one logical line into `rows`, splitting on spaces. A word wider than `width` is
/// chunked by chars so it fits without overflow.
fn wrap_line(line: &str, width: usize, rows: &mut Vec<String>) {
    let mut current = String::new();
    for word in line.split(' ') {
        let word_chars = word.chars().count();
        if current.is_empty() {
            if word_chars <= width {
                current.push_str(word);
            } else {
                // Word alone exceeds the width: chunk it by chars.
                let mut chars = word.chars().peekable();
                while chars.peek().is_some() {
                    let chunk: String = chars.by_ref().take(width).collect();
                    rows.push(chunk);
                }
                continue;
            }
        } else if current.chars().count() + 1 + word_chars <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            rows.push(std::mem::take(&mut current));
            if word_chars <= width {
                current.push_str(word);
            } else {
                let mut chars = word.chars().peekable();
                while chars.peek().is_some() {
                    let chunk: String = chars.by_ref().take(width).collect();
                    rows.push(chunk);
                }
            }
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
}

#[cfg(test)]
mod tests {
    use super::hard_wrap;

    #[test]
    fn wraps_long_lines_and_preserves_blank_lines() {
        assert_eq!(hard_wrap("abcdef", 3), vec!["abc", "def"]);
        assert_eq!(hard_wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn word_wrap_breaks_at_spaces_and_chunks_long_words() {
        assert_eq!(
            hard_wrap("the quick brown fox", 5),
            vec!["the", "quick", "brown", "fox"]
        );
        assert_eq!(
            hard_wrap("a verylongword here", 4),
            vec!["a", "very", "long", "word", "here"]
        );
    }
}
