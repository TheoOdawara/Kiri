use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::modules::tui::domain::modal::{
    ApprovalOption, PendingApproval, PendingPlan, PlanOption,
};
use crate::modules::tui::infrastructure::markdown;
use crate::modules::tui::infrastructure::theme;

/// Render the tool-call confirmation: the proposed action plus its options.
pub fn render(pending: &PendingApproval, frame: &mut Frame, area: Rect) {
    let options: Vec<&str> = ApprovalOption::ALL.iter().map(|o| o.label()).collect();
    render_stanza(
        "aprovação",
        pending.action(),
        &options,
        pending.selected,
        frame,
        area,
    );
}

/// The borderless confirmation stanza. No box, no cage: an etched guilloché hairline sets it off from
/// the transcript above (containment is positional — its dedicated region just over the input — plus
/// perceptual, once rack-focus recedes the transcript). The action reads as a calm question, the options
/// hang with the `❯` caret carrying the single accent (the input cursor is suspended while a choice is
/// open). The stanza fills the region top-down; the layout reserved exactly its height.
fn render_stanza(
    label: &str,
    action: &str,
    options: &[&str],
    selected: usize,
    frame: &mut Frame,
    area: Rect,
) {
    frame.render_widget(Clear, area);

    let inner_w = area.width.max(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        hairline(inner_w),
        theme::base().fg(theme::GUILLOCHE),
    ));
    lines.push(Line::styled(format!(" {label}"), theme::dim()));
    lines.append(&mut markdown::render(action, theme::strong(), inner_w));
    lines.push(Line::default());
    for (i, option) in options.iter().enumerate() {
        let (marker, style) = super::option_marker(i == selected);
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(format!("{}. {option}", i + 1), style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines).style(theme::base()), area);
}

/// An etched hairline (`┄┈` motif) `width` cells wide — low-contrast steel, reads as engraved metal
/// rather than a dead dash run.
fn hairline(width: usize) -> String {
    "┄┈".chars().cycle().take(width).collect()
}

/// The action line shown above the plan options. Exposed so the view can size the reserved region
/// against the same text the stanza renders.
pub const PLAN_ACTION: &str = "Plano pronto. Escolha:";

/// The number of plan options, exposed so the view can size the reserved area without importing the
/// enum across module boundaries.
pub fn plan_options_len() -> usize {
    PlanOption::ALL.len()
}

/// The width and height the stanza occupies in `area`: full width, and a height of one hairline row, the
/// label, the wrapped action, a blank, and one row per option. Used by the view to reserve exactly the
/// rows the stanza needs at the bottom, so it never overlays the transcript or the plan text.
pub fn box_dims(area: Rect, action: &str, option_count: usize) -> (u16, u16) {
    let inner_w = area.width.max(1) as usize;
    let action_rows = markdown::render(action, theme::strong(), inner_w)
        .len()
        .max(1);
    // hairline + label + action rows + blank + options
    let height = ((2 + action_rows + 1 + option_count) as u16).min(area.height.max(1));
    (area.width, height)
}

/// Render the plan stanza into its pre-reserved region (the bottom slice just above the input), so the
/// plan text stays visible above it.
pub fn render_plan_into(plan: &PendingPlan, frame: &mut Frame, area: Rect) {
    let options: Vec<&str> = PlanOption::ALL.iter().map(|o| o.label()).collect();
    render_stanza("plano", PLAN_ACTION, &options, plan.selected, frame, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::infrastructure::text::display_width;

    #[test]
    fn hairline_is_exactly_width_cells() {
        assert_eq!(display_width(&hairline(0)), 0);
        assert_eq!(display_width(&hairline(5)), 5);
        assert_eq!(hairline(4), "┄┈┄┈");
    }
}
