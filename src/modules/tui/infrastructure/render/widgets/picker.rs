use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::modules::tui::domain::picker::Picker;
use crate::modules::tui::infrastructure::theme;

/// Render a generic single-choice picker as a centered search-modal.
pub fn render(picker: &Picker, frame: &mut Frame, area: Rect) {
    // 1. Center the modal on the screen.
    let width = 64.min(area.width.saturating_sub(4)).max(20);
    let height = 15.min(area.height.saturating_sub(2)).max(8);
    let modal_area = centered_rect(width, height, area);

    // 2. Clear the area under the modal so the transcript behind doesn't bleed through.
    frame.render_widget(Clear, modal_area);

    // 3. Create the border block.
    let title = format!(" Escolha {} ", picker.label);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::STEEL_RAMP[3])) // Steel border
        .title(Line::styled(title, theme::strong()))
        .style(theme::base());

    let inner_area = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // 4. Split inner area into: Search bar (1 row), separator (1 row), options list (rest), footer (1 row).
    let chunks = Layout::vertical([
        Constraint::Length(1), // Search Input row
        Constraint::Length(1), // Separator line
        Constraint::Min(1),    // List of options
        Constraint::Length(1), // Hint footer
    ])
    .split(inner_area);

    // 5. Render Search Input row
    let search_label = Span::styled("  Buscar: ", theme::dim());
    let search_val = Span::styled(&picker.query, theme::strong());
    let cursor = Span::styled("▊", theme::accent());
    let search_line = Line::from(vec![search_label, search_val, cursor]);
    frame.render_widget(Paragraph::new(search_line).style(theme::base()), chunks[0]);

    // 6. Render Separator Line
    let sep_width = chunks[1].width as usize;
    let separator = "─".repeat(sep_width);
    frame.render_widget(
        Paragraph::new(Line::styled(separator, Style::default().fg(theme::STEEL_RAMP[4]))),
        chunks[1],
    );

    // 7. Render Options List
    let filtered = picker.filtered_options();
    let mut lines = Vec::new();
    let list_height = chunks[2].height as usize;
    
    if filtered.is_empty() {
        lines.push(Line::styled("  (nenhum resultado encontrado)", theme::dim()));
    } else {
        // We have `picker.selected` as the index in the `filtered` list.
        // Scroll the viewport window if necessary.
        let start_index = if picker.selected >= list_height {
            picker.selected - list_height + 1
        } else {
            0
        };
        let end_index = (start_index + list_height).min(filtered.len());
        
        for idx in start_index..end_index {
            let (_, option_text) = &filtered[idx];
            let is_selected = idx == picker.selected;
            let (marker, style) = super::option_marker(is_selected);
            lines.push(Line::from(vec![
                Span::styled(marker, style),
                Span::styled(*option_text, style),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines).style(theme::base()), chunks[2]);

    // 8. Render Hint Footer
    let footer_text = " Esc: fechar · ↑↓: navegar · Enter: selecionar";
    frame.render_widget(
        Paragraph::new(Line::styled(footer_text, theme::dim())),
        chunks[3],
    );
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}
