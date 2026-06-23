use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// Render the bottom line: the pending-approval prompt when one is waiting, otherwise context-sensitive
/// keybinding hints.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let line = if let Some(pending) = &model.pending_approval {
        let suffix = if pending.default_accept {
            "[S/n]"
        } else {
            "[s/N]"
        };
        Line::from(vec![
            Span::styled(
                " aprovar? ",
                theme::base()
                    .fg(theme::WARNING)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::styled(format!(" {} {suffix}", pending.prompt), theme::base()),
        ])
    } else if model.busy {
        Line::styled("  ^C cancela · streaming…", theme::dim())
    } else {
        Line::styled(
            "  Enter envia · ⇧Tab modo · Alt+Enter nova linha · ↑↓ histórico · PgUp/PgDn rola · ^C/^D sai · /exit",
            theme::dim(),
        )
    };
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
