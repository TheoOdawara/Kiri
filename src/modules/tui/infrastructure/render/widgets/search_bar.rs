use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};

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

    // prefix | query editor | match count — the editor owns the real caret.
    let suffix_w = matches_text.chars().count() as u16;
    let prefix_w = 11u16; // "   busca › "
    let [prefix_area, query_area, suffix_area] = Layout::horizontal([
        Constraint::Length(prefix_w),
        Constraint::Min(1),
        Constraint::Length(suffix_w.min(region.width.saturating_sub(prefix_w))),
    ])
    .areas(region);

    frame.render_widget(
        Paragraph::new(Line::styled("   busca › ", theme::dim())),
        prefix_area,
    );
    frame.render_widget(query.widget(), query_area);
    frame.render_widget(
        Paragraph::new(Line::styled(matches_text, theme::dim())),
        suffix_area,
    );
}
