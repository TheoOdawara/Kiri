use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::agent::application::approval_policy::ApprovalMode;
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// The forged rule directly above the input: tsuba node caps (`◈`) frame a steel rule that carries the
/// model, workspace, and active approval mode, with the spinner and elapsed seconds pinned right while a
/// turn runs. This is the "régua forjada" that seats the input cluster next to where the user types.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let width = area.width as usize;
    let context = format!("{} · {}", model.status.model, model.status.workspace);
    let (mode_label, mode_style) = mode_badge(model.approval_mode);
    let right = if model.busy {
        let glyph = theme::SPINNER[model.status.spinner_frame % theme::SPINNER.len()];
        format!("{glyph} {}s", model.status.elapsed_secs)
    } else {
        String::new()
    };

    // Layout: "◈─ " context " · " MODE " " ──fill── [right " "] "─◈". The dashes run continuously into
    // the closing tsuba cap; when a turn runs, the spinner + elapsed sit just left of the cap.
    let head = "◈─ ".chars().count()
        + context.chars().count()
        + " · ".chars().count()
        + mode_label.chars().count()
        + 1; // trailing space after the mode badge
    let tail = if right.is_empty() {
        "─◈".chars().count()
    } else {
        right.chars().count() + 1 + "─◈".chars().count() // right + space + cap
    };
    let fill = width.saturating_sub(head + tail);

    let mut spans = vec![
        Span::styled("◈─ ", theme::dim()),
        Span::styled(context, theme::dim()),
        Span::styled(" · ", theme::dim()),
        Span::styled(mode_label, mode_style),
        Span::styled(" ", theme::dim()),
        Span::styled("─".repeat(fill), theme::dim()),
    ];
    if !right.is_empty() {
        spans.push(Span::styled(right, theme::accent()));
        spans.push(Span::styled(" ", theme::dim()));
    }
    spans.push(Span::styled("─◈", theme::dim()));

    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme::base()), area);
}

/// The approval-mode badge: label + style. Default is dim; Auto warns (yellow); Plan accents (cyan).
fn mode_badge(mode: ApprovalMode) -> (&'static str, Style) {
    match mode {
        ApprovalMode::Default => ("DEFAULT", theme::dim()),
        ApprovalMode::Auto => (
            "AUTO",
            theme::base()
                .fg(theme::WARNING)
                .add_modifier(Modifier::BOLD),
        ),
        ApprovalMode::Plan => (
            "PLAN",
            theme::base()
                .fg(theme::HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        ),
    }
}
