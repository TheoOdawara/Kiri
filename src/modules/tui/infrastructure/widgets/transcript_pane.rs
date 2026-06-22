use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::splash;

/// Render the scrolling transcript. Items are pre-wrapped to the pane width (so scroll offsets are
/// exact line counts), then scrolled so the newest content is pinned to the bottom unless the user has
/// scrolled up (`scrollback`). When the conversation is empty, the brand splash takes the pane instead.
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
        TranscriptItem::Reasoning(text) => push_wrapped(text, width, theme::dim(), out),
        TranscriptItem::Assistant(text) => {
            push_wrapped(text, width, Style::default().fg(theme::STEEL), out)
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

/// Wrap by char count (display-width-approximate, fine for a transcript): split on newlines, then
/// chunk each logical line to the pane width.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for logical in text.split('\n') {
        let chars: Vec<char> = logical.chars().collect();
        if chars.is_empty() {
            rows.push(String::new());
            continue;
        }
        let mut i = 0;
        while i < chars.len() {
            let end = (i + width).min(chars.len());
            rows.push(chars[i..end].iter().collect());
            i = end;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::hard_wrap;

    #[test]
    fn wraps_long_lines_and_preserves_blank_lines() {
        assert_eq!(hard_wrap("abcdef", 3), vec!["abc", "def"]);
        assert_eq!(hard_wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }
}
