use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::text::display_width;
use crate::modules::tui::infrastructure::theme;
use crate::shared::kernel::approval_mode::ApprovalMode;

/// One empty cell kept at the line's end so a glyph a terminal renders wider than its measured width
/// (the ambiguous-width brand caps) nudges into the slack instead of pushing the closing cap off-screen.
const SAFETY: usize = 1;

/// The forged rule directly above the input: tsuba node caps (`◈`) frame a steel rule that carries the
/// model, workspace, and active approval mode, with the spinner and elapsed seconds pinned right while a
/// turn runs. This is the "régua forjada" that seats the input cluster next to where the user types.
/// The line is responsive: on narrow terminals the context is abbreviated (workspace first, then model)
/// and the fill dashes shrink, so the caps never overflow the width.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let width = area.width as usize;
    let (mode_label, mode_style) = mode_badge(model.approval_mode);
    let right = if model.busy {
        let glyph = theme::SPINNER[model.status.spinner_frame % theme::SPINNER.len()];
        format!("{glyph} {}", model.status.elapsed_label())
    } else {
        String::new()
    };

    // Build the context, shrinking it to fit. Prefer keeping the workspace; drop the model first, then
    // abbreviate the workspace with an ellipsis if the line still does not fit.
    let full = format!("{} · {}", model.status.model, model.status.workspace);
    let context = fit_context(
        &full,
        &model.status.workspace,
        width,
        display_width(&right),
        mode_label,
    );

    // Layout: "◈─ " context " · " MODE " " ──fill── [right " "] "─◈". The dashes run continuously into
    // the closing tsuba cap; when a turn runs, the spinner + elapsed sit just left of the cap. Widths
    // are measured in display cells (not chars) so wide glyphs do not push the closing cap off-screen.
    let head = display_width("◈─ ")
        + display_width(&context)
        + display_width(" · ")
        + display_width(mode_label)
        + 1; // trailing space after the mode badge
    let tail = if right.is_empty() {
        display_width("─◈")
    } else {
        display_width(&right) + 1 + display_width("─◈") // right + space + cap
    };
    let fill = width.saturating_sub(head + tail + SAFETY);

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

/// Pick the context string that fits the available width. The fixed overhead is the caps, separators,
/// mode badge, the right-aligned spinner block, and a one-cell safety margin; whatever remains is the
/// context budget. Falls back to the workspace alone, then to its tail (the actual folder, prefixed
/// with `…`), and finally to an empty string. Widths are measured in display cells.
fn fit_context(
    full: &str,
    workspace: &str,
    width: usize,
    right_len: usize,
    mode_label: &str,
) -> String {
    const HEAD: usize = 3; // "◈─ "
    const SEP: usize = 3; // " · "
    const MODE_PAD: usize = 1; // trailing space after mode badge
    const TAIL: usize = 2; // "─◈"
    let right_block = if right_len > 0 { right_len + 1 } else { 0 }; // right text + its trailing space
    let overhead = HEAD + SEP + display_width(mode_label) + MODE_PAD + right_block + TAIL + SAFETY;
    let budget = width.saturating_sub(overhead);
    if display_width(full) <= budget {
        return full.to_string();
    }
    if display_width(workspace) <= budget {
        return workspace.to_string();
    }
    if budget <= 1 {
        return String::new();
    }
    // Keep the tail of the workspace — the working directory's name is more useful than its drive
    // prefix. Reserve one cell for the leading ellipsis.
    let chars: Vec<char> = workspace.chars().collect();
    let keep = budget.saturating_sub(1).min(chars.len());
    let start = chars.len() - keep;
    let tail: String = chars[start..].iter().collect();
    format!("…{tail}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_context_keeps_the_workspace_tail_when_truncating() {
        let workspace = "C:/Users/dev/projects/kiri";
        let full = format!("model · {workspace}");
        // A narrow width forces truncation; the tail (the folder) must survive, prefixed with `…`.
        let ctx = fit_context(&full, workspace, 24, 0, "AUTO");
        assert!(
            ctx.starts_with('…'),
            "should ellipsize from the front: {ctx}"
        );
        assert!(ctx.ends_with("kiri"), "should keep the folder tail: {ctx}");
    }

    #[test]
    fn fit_context_prefers_the_full_string_when_it_fits() {
        let workspace = "/w";
        let full = "m · /w";
        assert_eq!(fit_context(full, workspace, 80, 0, "AUTO"), full);
    }
}
