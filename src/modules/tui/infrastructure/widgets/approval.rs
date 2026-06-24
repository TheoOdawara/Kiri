use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::modules::tui::domain::view_state::{
    APPROVAL_OPTIONS, PLAN_OPTIONS, PendingApproval, PendingPlan,
};
use crate::modules::tui::infrastructure::theme;

/// Render the tool-call confirmation box: the proposed action plus its options.
pub fn render(pending: &PendingApproval, frame: &mut Frame, area: Rect) {
    render_box(
        " aprovação ",
        pending.action(),
        &APPROVAL_OPTIONS,
        pending.selected,
        frame,
        area,
    );
}

/// Render the plan box shown after a plan-mode turn: the plan itself is above in the transcript, so the
/// box only asks what to do with it.
pub fn render_plan(plan: &PendingPlan, frame: &mut Frame, area: Rect) {
    render_box(
        " plano ",
        "Plano pronto. Escolha:",
        &PLAN_OPTIONS,
        plan.selected,
        frame,
        area,
    );
}

/// The shared market-standard choice box (Claude/Codex/Copilot-CLI): an overlay anchored to the bottom
/// of `area`, with the action line and selectable options, the current one highlighted.
fn render_box(
    title: &str,
    action: &str,
    options: &[&str],
    selected: usize,
    frame: &mut Frame,
    area: Rect,
) {
    let region = box_rect(area, options.len());
    frame.render_widget(Clear, region);

    let mut lines: Vec<Line> = vec![
        Line::styled(
            action.to_string(),
            Style::default()
                .fg(theme::STEEL)
                .add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    for (i, option) in options.iter().enumerate() {
        let (marker, style) = if i == selected {
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
        .title(title.to_string())
        .style(theme::base());
    frame.render_widget(Paragraph::new(lines).block(block), region);
}

/// A centered box pinned to the bottom of `area`, sized to fit the action line plus the option list
/// (action + blank + options + two border rows).
fn box_rect(area: Rect, option_count: usize) -> Rect {
    let height = (option_count as u16 + 4).min(area.height.max(1));
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
