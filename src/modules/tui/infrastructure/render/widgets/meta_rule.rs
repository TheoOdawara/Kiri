use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::text::display_width;
use crate::modules::tui::infrastructure::theme;
use crate::shared::kernel::approval_mode::ApprovalMode;

/// One empty cell kept at the line's end so a glyph a terminal renders wider than its measured width
/// (the ambiguous-width brand caps) nudges into the slack instead of pushing the closing cap off-screen.
const SAFETY: usize = 1;

/// The quiet status spine directly above the input — the retired dash-rail. No rule, no caps strung on
/// dashes: the context (`model · workspace`) sits dim at the left, the approval mode is ghosted right
/// behind a single `◈` tsuba anchor, and only void breathes between them. While a turn runs the spinner
/// and elapsed seconds take the one accent just left of the mode; once a turn ends in error, the same
/// slot instead holds a persistent "✗ erro" badge (issue #8b) until the next turn begins — unlike the
/// transcript's own error `Notice`, which scrolls out of view as more content is appended, this stays put
/// so a failure is never missed just because more text landed after it. Responsive: as width shrinks the
/// workspace is abbreviated then dropped, but the mode anchor always survives.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let width = area.width as usize;
    let (mode_label, mode_style) = mode_badge(model.approval_mode);
    let (right_content, right_style) = if model.busy {
        let glyph = theme::spinner_glyph(model.status.spinner_frame);
        (
            format!("{glyph} {}", model.status.elapsed_label()),
            theme::accent(),
        )
    } else if model.status.turn_failed {
        ("✗ erro".to_string(), Style::default().fg(theme::ERROR))
    } else {
        // The style is never read: `right_text` (built below) is empty, so the span carrying it is
        // skipped entirely — `Style::default()` makes that explicit rather than reusing `accent()`.
        (String::new(), Style::default())
    };

    // Build the context, shrinking it to fit. Prefer keeping the workspace; drop the model first, then
    // abbreviate the workspace with an ellipsis if the line still does not fit.
    let full = format!("{} · {}", model.status.model, model.status.workspace);
    let context = fit_context(
        &full,
        &model.status.workspace,
        width,
        display_width(&right_content),
        mode_label,
    );

    // Layout: context …void fill… [right ]MODE ◈ . The right cluster is the only thing pinned to the
    // edge; the space between is the negative space (間) that kills the old boxed rule. Widths are
    // measured in display cells so a wide glyph never pushes the anchor off-screen.
    let right_text = if right_content.is_empty() {
        String::new()
    } else {
        format!("{right_content} ")
    };
    let head = display_width(&context);
    let tail = display_width(&right_text) + display_width(mode_label) + display_width(" ◈");
    let fill = width.saturating_sub(head + tail + SAFETY).max(1);

    let mut spans = vec![
        Span::styled(context, theme::dim()),
        Span::styled(" ".repeat(fill), theme::base()),
    ];
    if !right_text.is_empty() {
        spans.push(Span::styled(right_text, right_style));
    }
    spans.push(Span::styled(mode_label, mode_style));
    spans.push(Span::styled(" ◈", theme::dim()));

    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme::base()), area);
}

/// Pick the context string that fits the available width. The fixed overhead is the minimum void fill,
/// the right cluster (optional spinner block + mode label), the `◈` anchor, and a one-cell safety
/// margin; whatever remains is the context budget. Falls back to the workspace alone, then to its tail
/// (the actual folder, prefixed with `…`), and finally to an empty string — the mode anchor always
/// survives. Widths are measured in display cells.
fn fit_context(
    full: &str,
    workspace: &str,
    width: usize,
    right_len: usize,
    mode_label: &str,
) -> String {
    const MIN_FILL: usize = 1; // at least one cell of void between context and the right cluster
    const ANCHOR: usize = 2; // " ◈"
    let right_block = if right_len > 0 { right_len + 1 } else { 0 }; // spinner block + its trailing space
    let overhead = MIN_FILL + right_block + display_width(mode_label) + ANCHOR + SAFETY;
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
        ApprovalMode::Auto => ("AUTO", theme::base().fg(theme::WARNING)),
        ApprovalMode::Plan => ("PLAN", theme::base().fg(theme::HIGHLIGHT)),
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
