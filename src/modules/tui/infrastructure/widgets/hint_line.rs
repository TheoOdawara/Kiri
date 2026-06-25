use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::text::display_width;
use crate::modules::tui::infrastructure::theme;

/// Render the bottom line: approval navigation while a confirmation is pending (the box shows the
/// options), the cancel hint while a turn runs, otherwise the editing/keybinding hints. The idle hint
/// collapses to a short form on narrow terminals so it never overflows.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let line = if model.pending_approval.is_some() {
        Line::styled(
            "  ↑↓ seleciona · Enter confirma · Esc recusa · ^C encerra",
            theme::dim(),
        )
    } else if model.pending_plan.is_some() {
        Line::styled(
            "  ↑↓ seleciona · Enter confirma · Esc cancela · ^C encerra",
            theme::dim(),
        )
    } else if model.busy {
        Line::styled("  ^C cancela · Esc 2× cancela · streaming…", theme::dim())
    } else {
        // Pick the longest hint variant that fits the width, so nothing is cut on narrow terminals.
        let variants = [
            "  Enter envia · ⇧Tab modo · Alt+Enter nova linha · ↑↓ histórico · ⇧↑↓/PgUp/PgDn rola · ^O expande · ^Home/^End topo/fundo · ^C 2×/^D sai · /help",
            "  Enter envia · ⇧Tab modo · ↑↓ histórico · PgUp/PgDn rola · ^O expande · ^C 2×/^D sai · /help",
            "  Enter envia · ^C 2× sai · /help",
            "  Enter · ^C 2× · /help",
        ];
        let w = area.width as usize;
        let text = variants
            .iter()
            .find(|v| display_width(v) <= w)
            .copied()
            .unwrap_or("  Enter · /help");
        Line::styled(text, theme::dim())
    };
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
