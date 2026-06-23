use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::modules::tui::domain::command_menu::CommandMenu;
use crate::modules::tui::infrastructure::theme;

/// Render the live slash-command preview as an overlay anchored just above the input editor. Each row
/// pairs the canonical name with its short blurb; the highlighted row uses the cyan accent.
pub fn render(menu: &CommandMenu, frame: &mut Frame, anchor: Rect) {
    if menu.is_empty() {
        return;
    }
    let region = box_rect(anchor, menu.filtered().len());
    frame.render_widget(Clear, region);

    let mut lines: Vec<Line> = Vec::new();
    for (row, &cmd_index) in menu.filtered().iter().enumerate() {
        let spec = &crate::modules::tui::domain::command_menu::COMMANDS[cmd_index];
        let (marker, style) = if row == menu.selected() {
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
            Span::styled(spec.name, style),
            Span::styled("  ", style),
            Span::styled(spec.blurb, style),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::base().fg(theme::HIGHLIGHT))
        .title(" comandos ".to_string())
        .style(theme::base());
    frame.render_widget(Paragraph::new(lines).block(block), region);
}

/// Anchor the box to the top edge of `anchor` (the input region), so the popup floats just above the
/// editor. The available height is the vertical space *above* the editor (`anchor.y`); height is the
/// smaller of the desired rows+2 and that space. Width is the smaller of `anchor.width` and 64,
/// left-aligned with the editor column.
fn box_rect(anchor: Rect, row_count: usize) -> Rect {
    let desired = row_count as u16 + 2;
    let height = desired.min(anchor.y.max(1));
    let width = anchor.width.min(64);
    let y = anchor.y.saturating_sub(height);
    Rect {
        x: anchor.x,
        y,
        width,
        height,
    }
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
        let menu = CommandMenu::open("/");
        let out = paint(&menu, 64, 16);
        assert!(out.contains("comandos"), "title missing:\n{out}");
        assert!(out.contains("/new"), "canonical names missing:\n{out}");
        assert!(out.contains("❯"), "highlight marker missing:\n{out}");
    }

    #[test]
    fn empty_menu_renders_nothing() {
        let menu = CommandMenu::open("/zzz");
        let out = paint(&menu, 64, 16);
        assert!(
            !out.contains("comandos"),
            "empty menu must not render:\n{out}"
        );
    }
}
