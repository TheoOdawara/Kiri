use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Motion;
use crate::modules::tui::infrastructure::theme;

/// The breath-in: each content row forges in from the void to its final colour, staggered top-to-bottom,
/// then the splash freezes byte-identical.
const STAGGER_MS: u128 = 18; // delay added per row, so the seal forges in top-down
const FADE_MS: f32 = 220.0; // how long one row takes to reach its final colour
const SETTLE_MS: u128 = 600; // after this the whole splash is settled and still

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
/// pane is too small, leaving the mark and boot line. On open the rows forge in from the void to steel,
/// staggered top-to-bottom, then freeze; a keypress (which backdates `opened_at`) fast-forwards it, and
/// reduced motion skips it entirely. Content and position are unchanged — only ignited.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    opened_at: Option<Instant>,
    now: Option<Instant>,
    motion: Motion,
) {
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
    lines.push(Line::styled("[ KIRI ]", theme::strong()));
    lines.push(Line::from(vec![
        Span::styled("KIRI harness system: Protecting codebase... ", theme::dim()),
        Span::styled("[OK]", theme::base().fg(theme::SUCCESS)),
    ]));
    lines.push(Line::default());
    lines.push(Line::styled(
        "Forged from Tradition, Built for Precision",
        theme::dim(),
    ));

    if let Some(age_ms) = breath_age_ms(opened_at, now, motion) {
        for (row, line) in lines.iter_mut().enumerate() {
            ignite_row(line, row_progress(age_ms, row));
        }
    }

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

/// The splash age in ms while the breath-in is still playing, or `None` once it has settled (or motion is
/// reduced / the clock is unstamped) — the caller then renders the final, still frame.
fn breath_age_ms(opened_at: Option<Instant>, now: Option<Instant>, motion: Motion) -> Option<u128> {
    if motion.is_reduced() {
        return None;
    }
    let age = now?.saturating_duration_since(opened_at?).as_millis();
    (age < SETTLE_MS).then_some(age)
}

/// How fully row `row` has forged in: zero until its staggered start, ramping to one over `FADE_MS`.
fn row_progress(age_ms: u128, row: usize) -> f32 {
    let start = row as u128 * STAGGER_MS;
    (age_ms.saturating_sub(start) as f32 / FADE_MS).clamp(0.0, 1.0)
}

/// Lerp every span of a row from the void toward its final colour by `t` — at `t = 0` it is invisible
/// (void on void), at `t = 1` it is its designed colour.
fn ignite_row(line: &mut Line<'static>, t: f32) {
    for span in &mut line.spans {
        let final_fg = span.style.fg.unwrap_or(theme::STEEL);
        span.style = span.style.fg(theme::ramp(&[theme::VOID, final_fg], t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn breath_plays_then_settles() {
        let o = Instant::now();
        assert_eq!(breath_age_ms(Some(o), Some(o), Motion::Full), Some(0));
        // Past the settle window the splash freezes (no age → final, still frame).
        assert_eq!(
            breath_age_ms(Some(o), Some(o + Duration::from_millis(600)), Motion::Full),
            None
        );
        // Reduced motion never plays it.
        assert_eq!(breath_age_ms(Some(o), Some(o), Motion::Reduced), None);
    }

    #[test]
    fn row_progress_staggers_and_clamps() {
        assert_eq!(
            row_progress(0, 5),
            0.0,
            "a later row has not started at age 0"
        );
        assert_eq!(
            row_progress(10_000, 0),
            1.0,
            "a long-elapsed row is fully lit"
        );
    }
}
