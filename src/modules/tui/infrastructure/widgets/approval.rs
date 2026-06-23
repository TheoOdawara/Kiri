use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::modules::tui::domain::view_state::{APPROVAL_OPTIONS, PendingApproval};
use crate::modules::tui::infrastructure::theme;

/// Render the rich approval box as an overlay anchored to the bottom of `area` (the transcript pane):
/// the proposed action, then the selectable options with the current one highlighted — the
/// market-standard confirmation pattern (Claude/Codex/Copilot-CLI).
pub fn render(pending: &PendingApproval, frame: &mut Frame, area: Rect) {
    let region = box_rect(area);
    frame.render_widget(Clear, region);

    let mut lines: Vec<Line> = vec![
        Line::styled(
            pending.action().to_string(),
            Style::default()
                .fg(theme::STEEL)
                .add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    for (i, option) in APPROVAL_OPTIONS.iter().enumerate() {
        let (marker, style) = if i == pending.selected {
            (
                "❯ ",
                theme::base()
                    .fg(theme::HIGHLIGHT)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("  ", theme::dim())
        };
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(format!("{}. {option}", i + 1), style),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::base().fg(theme::WARNING))
        .title(" aprovação ")
        .style(theme::base());
    frame.render_widget(Paragraph::new(lines).block(block), region);
}

/// A centered box pinned to the bottom of `area`, sized to fit the action line plus the option list
/// (action + blank + options + two border rows).
fn box_rect(area: Rect) -> Rect {
    let height = (APPROVAL_OPTIONS.len() as u16 + 4).min(area.height.max(1));
    let width = area.width.saturating_sub(4).clamp(1, 64);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height);
    Rect {
        x,
        y,
        width,
        height,
    }
}
