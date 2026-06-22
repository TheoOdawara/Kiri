use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::Text;
use ratatui::widgets::{Block, Paragraph};

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// Render the multi-line input editor in a bordered block, placing the hardware cursor at the buffer's
/// cursor when the editor is active (no pending approval, not mid-turn).
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let active = model.pending_approval.is_none() && !model.busy;
    let title = if model.busy {
        " você (ocupado) "
    } else {
        " você "
    };
    let block = Block::bordered().title(title).border_style(if active {
        theme::accent()
    } else {
        theme::dim()
    });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(Text::raw(model.input.text())), inner);

    if active {
        let (col, row) = cursor_position(model.input.text(), model.input.cursor());
        frame.set_cursor_position(Position {
            x: inner.x + col.min(inner.width.saturating_sub(1)),
            y: inner.y + row.min(inner.height.saturating_sub(1)),
        });
    }
}

/// Column and row of the cursor within the buffer (rows separated by newlines; no soft-wrap accounted).
fn cursor_position(text: &str, cursor: usize) -> (u16, u16) {
    let before = &text[..cursor];
    let row = before.matches('\n').count() as u16;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() as u16;
    (col, row)
}
