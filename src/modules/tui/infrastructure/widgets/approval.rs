use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::modules::tui::domain::view_state::{
    APPROVAL_OPTIONS, PLAN_OPTIONS, PendingApproval, PendingPlan,
};
use crate::modules::tui::infrastructure::markdown;
use crate::modules::tui::infrastructure::text::display_width;
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

/// The shared market-standard choice box (Claude/Codex/Copilot-CLI): an overlay anchored to the bottom
/// of `area`, with the action line and selectable options, the current one highlighted. The action wraps
/// to the box width so long commands never overflow.
fn render_box(
    title: &str,
    action: &str,
    options: &[&str],
    selected: usize,
    frame: &mut Frame,
    area: Rect,
) {
    let region = box_rect(area, action, options.len());
    frame.render_widget(Clear, region);

    // Render the action as markdown (bold/code spans survive), wrapped to the inner width.
    let inner_w = region.width.saturating_sub(2).max(1) as usize;
    let base = Style::default()
        .fg(theme::STEEL)
        .add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = markdown::render(action, base, inner_w);
    lines.push(Line::default());
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

/// A centered box pinned to the bottom of `area`, sized to fit the rendered action line plus the
/// option list (action rows + blank + options + two border rows), clamped to the available height.
/// Width grows with the action up to a cap, leaving room on narrow terminals.
fn box_rect(area: Rect, action: &str, option_count: usize) -> Rect {
    let (width, height) = box_dims(area, action, option_count);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height);
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// The action line shown above the plan options. Exposed so the view can size the reserved box region
/// against the same text the box renders.
pub const PLAN_ACTION: &str = "Plano pronto. Escolha:";

/// The number of plan options, exposed so the view can size the reserved area without importing the
/// constant across module boundaries.
pub fn plan_options_len() -> usize {
    PLAN_OPTIONS.len()
}

/// Horizontal padding (columns per side) kept around the plan box, and the width bounds it clamps to.
const BOX_H_PADDING: u16 = 4;
const BOX_MIN_WIDTH: u16 = 1;
const BOX_MAX_WIDTH: u16 = 64;
/// Extra columns added to the action text width for the box's left/right borders.
const BOX_BORDER_COLS: u16 = 2;

/// The width and height the box would occupy in `area`, without positioning it. Used by the view to
/// reserve space at the bottom of the transcript so the plan box never overlays the plan text.
pub fn box_dims(area: Rect, action: &str, option_count: usize) -> (u16, u16) {
    let max_w = area
        .width
        .saturating_sub(BOX_H_PADDING)
        .clamp(BOX_MIN_WIDTH, BOX_MAX_WIDTH);
    // Desired width: the longest unwrapped logical line of the action, capped; options are short.
    let action_w = action.split('\n').map(display_width).max().unwrap_or(0);
    let width = max_w
        .max(action_w.min(BOX_MAX_WIDTH as usize) as u16 + BOX_BORDER_COLS)
        .min(area.width.max(1));
    let inner_w = width.saturating_sub(2).max(1) as usize;
    let base = Style::default()
        .fg(theme::STEEL)
        .add_modifier(Modifier::BOLD);
    let action_rows = markdown::render(action, base, inner_w).len().max(1);
    let height = ((action_rows + 1 + option_count + 2) as u16) // action + blank + options + borders
        .min(area.height.max(1));
    (width, height)
}

/// Render the plan box into a pre-reserved area (the view hands it the bottom slice of the transcript
/// region, so the plan stays visible above). Unlike `render`, this positions the box at the top of the
/// given area instead of pinning it to the bottom.
pub fn render_plan_into(plan: &PendingPlan, frame: &mut Frame, area: Rect) {
    let action = PLAN_ACTION;
    let (width, height) = box_dims(area, action, PLAN_OPTIONS.len());
    let region = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y, // top of the reserved area
        width,
        height,
    };
    render_box(
        " plano ",
        action,
        &PLAN_OPTIONS,
        plan.selected,
        frame,
        region,
    );
}
