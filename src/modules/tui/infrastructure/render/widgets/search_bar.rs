use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, Paragraph};
use ratatui::text::{Line, Span};

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// Render the active search bar floating just above the input editor.
pub fn render(model: &Model, frame: &mut Frame, anchor: Rect) {
    let Some(query) = &model.search_query else {
        return;
    };

    // Sit exactly 1 row above the editor input.
    let y = anchor.y.saturating_sub(1);
    let width = anchor.width.min(64);
    let region = Rect {
        x: anchor.x,
        y,
        width,
        height: 1,
    };

    frame.render_widget(Clear, region);

    let prefix = "   busca › ";
    let matches_text = if model.search_results.is_empty() {
        if query.is_empty() {
            " (digite para buscar)".to_string()
        } else {
            " (nenhuma correspondência)".to_string()
        }
    } else {
        format!(
            " (correspondência {} de {})",
            model.active_search_match + 1,
            model.search_results.len()
        )
    };

    let mut spans = vec![
        Span::styled(prefix, theme::dim()),
        Span::styled(query.clone(), theme::strong()),
        Span::styled(matches_text, theme::dim()),
    ];

    // Pad with spaces to clear the line background
    let line_len: usize = spans.iter().map(|s| s.content.len()).sum();
    if line_len < region.width as usize {
        let padding = " ".repeat(region.width as usize - line_len);
        spans.push(Span::raw(padding));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme::base()), region);
}
