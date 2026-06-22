use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::infrastructure::theme;

/// The harness icon — the tsuba ring enclosing the three data-node hexagons — rendered in half-block from
/// `docs/marca/icon.png`. Every row is padded to `ICON_W` so center alignment keeps the ring coherent.
const ICON: &[&str] = &[
    "           ▄▄▄███████▄▄▄",
    "        ▄▄████████████████▄▄",
    "      ▄████▀▀   ▄██▄   ▀▀████▄",
    "    ▄████▀   ▄███████▄▄   ▀████",
    "   ▄███▀    ███████████     ▀███",
    "  ▄███      ███████████      ▀███",
    "  ███       ███████████       ████",
    " ████      ▄▄█▀██████▀▄▄▄      ███",
    " ████  ▄▄██████▄▄▀▀▄███████▄   ███",
    " ████  ██████████ ███████████  ███",
    "  ███  ██████████ ███████████ ▄███",
    "  ▀███▄██████████ ███████████▄███",
    "   ████▀▀██████▀▀  ▀███████▀████▀",
    "    ▀███▄ ▀▀▀▀        ▀▀▀  ▄███▀",
    "      ▀███▄             ▄▄███▀",
    "        ▀████▄▄▄▄▄▄▄▄▄▄████▀",
    "           ▀▀██████████▀▀",
];
const ICON_W: u16 = 34;

/// The empty-state splash, mirroring the CLI mock panel in `docs/marca/Design.png`: the harness icon, the
/// `[ KIRI ]` mark, a boot line with a green `[OK]` gate, and the tagline. The icon is dropped when the
/// pane is too small, leaving the mark and boot line.
pub fn render(frame: &mut Frame, area: Rect) {
    let show_icon = area.height as usize >= ICON.len() + 6 && area.width >= ICON_W;

    let mut lines: Vec<Line> = Vec::new();
    if show_icon {
        for row in ICON {
            lines.push(Line::styled(
                format!("{row:<w$}", w = ICON_W as usize),
                theme::base(),
            ));
        }
        lines.push(Line::default());
    }
    lines.push(Line::styled(
        "[ KIRI ]",
        theme::base().add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(vec![
        Span::styled("KIRI harness system: Protecting codebase... ", theme::dim()),
        Span::styled(
            "[OK]",
            theme::base()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::default());
    lines.push(Line::styled(
        "Forged from Tradition, Built for Precision",
        theme::dim(),
    ));

    let top_pad = (area.height as usize).saturating_sub(lines.len()) / 2;
    let mut padded = vec![Line::default(); top_pad];
    padded.extend(lines);

    frame.render_widget(
        Paragraph::new(padded)
            .alignment(Alignment::Center)
            .style(theme::base()),
        area,
    );
}
