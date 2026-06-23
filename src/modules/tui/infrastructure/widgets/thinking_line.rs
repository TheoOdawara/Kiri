use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// The live "thinking" line: the model's in-flight reasoning shown as a compact, scrolling line while a
/// turn runs. It clears when the turn ends, leaving the normal chat view. The transcript keeps its own
/// dim copy of the same reasoning, so this is a transient magnifier on the latest chunk, not a second
/// store. Showing it costs no extra tokens — the reasoning already arrives in the SSE stream.
///
/// Fallback: if the model emits no reasoning, after a 2-second grace period the line mirrors the
/// streaming content instead (with a distinct `✎` prefix) so the user always sees live activity while
/// a turn runs.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    if !model.busy {
        return;
    }
    let has_reasoning = !model.live_reasoning.is_empty();
    let elapsed = model
        .status
        .turn_started
        .map(|t| t.elapsed())
        .unwrap_or_default();
    let fallback = !has_reasoning && elapsed >= Duration::from_secs(2);
    if !has_reasoning && !fallback {
        return;
    }

    let (prefix, body_text) = if has_reasoning {
        ("💭 ", model.live_reasoning.as_str())
    } else {
        ("✎ ", model.live_content.as_str())
    };
    if body_text.is_empty() {
        return;
    }

    // Collapse newlines/whitespace runs so a multi-line delta reads as one flowing line.
    let flattened: String = body_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let width = area.width as usize;
    let prefix_cols = prefix.chars().count();
    let cap = width.saturating_sub(prefix_cols);
    let body = if flattened.chars().count() <= cap {
        flattened
    } else {
        // Keep the tail (the freshest words) and mark the elided head, mirroring a live tail.
        let chars: Vec<char> = flattened.chars().collect();
        let start = chars.len().saturating_sub(cap.saturating_sub(1));
        format!("…{}", chars[start..].iter().collect::<String>())
    };
    let line = Line::from(vec![
        Span::styled(prefix, theme::accent().add_modifier(Modifier::BOLD)),
        Span::styled(body, theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
