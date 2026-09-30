use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::modules::tui::domain::command_menu::CommandMenu;
use crate::modules::tui::infrastructure::text::{chunk_by_width, display_width};
use crate::modules::tui::infrastructure::theme;

/// Render the live slash-command preview as a subtle bordered dropdown box just above the input editor.
/// The selected row carries the single accent through its `❯` caret in the gutter; the rest stay dim.
pub fn render(menu: &CommandMenu, frame: &mut Frame, anchor: Rect) {
    if menu.is_empty() {
        return;
    }
    let region = box_rect(anchor, menu.len());
    frame.render_widget(Clear, region);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::STEEL_RAMP[4])) // Subtle dark steel border
        .title(Span::styled(" comandos ", theme::dim()))
        .style(theme::base());

    let inner_area = block.inner(region);
    frame.render_widget(block, region);

    let inner_w = inner_area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for row in 0..menu.len() {
        let Some(entry) = menu.row(row) else {
            continue;
        };
        let (marker, style) = super::option_marker(row == menu.selected());
        // Truncate the blurb so the row never overflows the list width.
        let name = entry.name();
        let aliases = entry.aliases();
        // The command's other names (issue #8c), dim and parenthesized right after the canonical one —
        // e.g. "/new (/novo)" — so a user typing the alias sees it is recognized without guessing.
        let alias_text = if aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", aliases.join(", "))
        };
        let name_cols = display_width(name);
        let alias_cols = display_width(&alias_text);
        let prefix_cols = 2 + name_cols + alias_cols + 2; // marker + name + aliases + gap
        let blurb_budget = inner_w.saturating_sub(prefix_cols);
        let blurb = truncate_blurb(entry.blurb(), blurb_budget);
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(name.to_string(), style),
            Span::styled(alias_text, theme::dim()),
            Span::styled("  ", style),
            Span::styled(blurb, style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines).style(theme::base()), inner_area);
}

/// Anchor the list to the top edge of `anchor` (the input region), so it floats just above the editor.
/// Height is the smaller of desired rows (command rows + 2 for borders) and the vertical space.
/// Width is the smaller of `anchor.width` and 64, left-aligned with the editor column.
fn box_rect(anchor: Rect, row_count: usize) -> Rect {
    let desired = row_count as u16 + 2; // top and bottom border rows
    let height = desired.min(anchor.y.max(2));
    let width = anchor.width.min(64);
    let y = anchor.y.saturating_sub(height);
    Rect {
        x: anchor.x,
        y,
        width,
        height,
    }
}

/// Truncate `blurb` to `budget` display columns, appending an ellipsis when it does not fit. A zero or
/// tiny budget yields an empty string rather than overflow.
fn truncate_blurb(blurb: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    if display_width(blurb) <= budget {
        return blurb.to_string();
    }
    let end = budget.saturating_sub(1); // reserve one cell for the ellipsis
    if end == 0 {
        return "…".to_string();
    }
    let prefix = chunk_by_width(blurb, end)
        .into_iter()
        .next()
        .unwrap_or_default();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::domain::command_menu::CommandMenu;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn paint(menu: &CommandMenu, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let anchor = Rect {
                    x: 0,
                    y: height.saturating_sub(3),
                    width,
                    height: 1,
                };
                render(menu, frame, anchor);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn menu_renders_title_and_highlighted_row() {
        let menu = CommandMenu::open("/", &[]);
        let out = paint(&menu, 64, 16);
        assert!(out.contains("comandos"), "title missing:\n{out}");
        assert!(out.contains("/new"), "canonical names missing:\n{out}");
        assert!(out.contains("❯"), "highlight marker missing:\n{out}");
    }

    #[test]
    fn menu_shows_aliases_next_to_the_canonical_name() {
        // Issue #8c: "/novo" for "/new" must be visible in the menu, not only accepted by the parser.
        let menu = CommandMenu::open("/new", &[]);
        let out = paint(&menu, 64, 16);
        assert!(out.contains("/new"), "canonical name missing:\n{out}");
        assert!(out.contains("/novo"), "alias missing:\n{out}");
    }

    #[test]
    fn empty_menu_renders_nothing() {
        let menu = CommandMenu::open("/zzz", &[]);
        let out = paint(&menu, 64, 16);
        assert!(
            !out.contains("comandos"),
            "empty menu must not render:\n{out}"
        );
    }
}
