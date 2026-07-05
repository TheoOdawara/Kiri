use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::markdown;
use crate::modules::tui::infrastructure::theme;

/// Render the proposed plan in a dedicated panel on the right side or full-screen.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let plan = match &model.pending_plan {
        Some(p) => p,
        None => return,
    };

    let inner_area_temp = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .inner(area);
    let width = inner_area_temp.width as usize;
    let style = Style::default().fg(theme::STEEL);
    let lines = markdown::render(&plan.plan, style, width);

    let total_lines = lines.len();
    let inner_height = inner_area_temp.height as usize;
    let max_scroll = total_lines.saturating_sub(inner_height);
    let scroll_offset = plan.scroll.min(max_scroll);

    let scroll_percentage = (scroll_offset * 100)
        .checked_div(max_scroll)
        .unwrap_or(100);

    let title_suffix = if max_scroll > 0 {
        format!(" [Rolar: ⇧↑/⇧↓/PgUp/PgDn | {}%] ", scroll_percentage)
    } else {
        "".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::WARNING))
        .title(Span::styled(
            format!(" 📋 PLANO PROPOSTO{} ", title_suffix),
            Style::default()
                .fg(theme::WARNING)
                .add_modifier(Modifier::BOLD),
        ));

    let inner_area = block.inner(area);

    // Draw the border block
    frame.render_widget(block, area);

    // Draw the scrolled text
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll_offset as u16, 0))
            .style(theme::base()),
        inner_area,
    );
}
