use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// Render the bottom line: approval navigation while a confirmation is pending (the box shows the
/// options), the cancel hint while a turn runs, otherwise the editing/keybinding hints.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let line = if model.pending_approval.is_some() {
        Line::styled(
            "  ↑↓ seleciona · Enter confirma · Esc recusa · ^C encerra",
            theme::dim(),
        )
    } else if model.busy {
        Line::styled("  ^C cancela · streaming…", theme::dim())
    } else {
        Line::styled(
            "  Enter envia · ⇧Tab modo · Alt+Enter nova linha · ↑↓ histórico · PgUp/PgDn rola · ^C/^D sai · /help",
            theme::dim(),
        )
    };
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
