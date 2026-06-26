use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::modules::tui::domain::model::{Model, Motion};
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::domain::view_state::InputBuffer;
use crate::modules::tui::infrastructure::theme::{self, GateState};

/// How long the gate's temper quench lasts after a turn settles.
const QUENCH_MS: f32 = 400.0;

/// Display columns of the prompt prefix `⬡ ›_ ` (gate glyph + space + `›` + `_` + space). The gate glyph
/// and `›` are width-1 in the terminals we target, so the text column stays aligned. Public so the view
/// can count wrapped input lines against the same column budget.
pub const PROMPT_COLS: u16 = 5;

/// Render the borderless input editor. A left gutter carries the state-driven gate seal and the `›_`
/// prompt on the first row; the `tui-textarea` widget renders the buffer in the column to its right, so
/// continuation rows align under the text. The widget owns editing, cursor, selection, and soft-wrap;
/// the runtime sets its theme styles once at startup. While a plan/approval box is up, the buffer is
/// drawn as plain text (no cursor) so its block cursor does not compete with the box's own highlight.
pub fn render(model: &Model, frame: &mut Frame, area: Rect, motion: Motion) {
    let state = gate_state(model);
    let (glyph, mut glyph_style) = theme::gate(state);
    // The reward beat: when a turn has just settled, the idle gate quenches from the busy cyan through
    // temper-blue into its resting colour — a "strike connected" before the UI goes silent.
    if matches!(state, GateState::Idle)
        && let Some(fg) = quench_fg(model.turn_settled_at, model.render_at, motion)
    {
        glyph_style = glyph_style.fg(fg);
    }
    let gutter_cols = PROMPT_COLS.min(area.width);

    let gutter = Rect {
        width: gutter_cols,
        ..area
    };
    let prompt = Line::from(vec![
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled("›_ ", theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(prompt).style(theme::base()), gutter);

    let editor_area = Rect {
        x: area.x + gutter_cols,
        width: area.width.saturating_sub(gutter_cols),
        ..area
    };
    let focused = model.pending_approval.is_none() && model.pending_plan.is_none();
    if focused {
        frame.render_widget(model.input.widget(), editor_area);
    } else {
        frame.render_widget(
            Paragraph::new(model.input.text())
                .style(theme::base())
                .wrap(Wrap { trim: false }),
            editor_area,
        );
    }
}

/// The gate's quench colour after a turn settles: `None` once the window has passed (the gate returns to
/// its resting idle colour) or when motion is reduced; otherwise the temper ramp applied to the settle
/// age. Pure, so the reward beat is unit-testable.
fn quench_fg(settled: Option<Instant>, now: Option<Instant>, motion: Motion) -> Option<Color> {
    if motion.is_reduced() {
        return None;
    }
    let age = now?.saturating_duration_since(settled?);
    let ms = age.as_millis() as f32;
    (ms < QUENCH_MS).then(|| theme::ramp(&theme::QUENCH_RAMP, ms / QUENCH_MS))
}

/// Resolve the editor's gate state, in priority order: a running turn, a pending approval, a trailing
/// error, an empty buffer, otherwise composing.
fn gate_state(model: &Model) -> GateState {
    if model.busy {
        GateState::Busy(model.status.spinner_frame)
    } else if model.pending_approval.is_some() {
        GateState::Approval
    } else if matches!(
        model.transcript.items().last(),
        Some(TranscriptItem::Notice(NoticeLevel::Error, _))
    ) {
        GateState::Error
    } else if model.input.is_empty() {
        GateState::Idle
    } else {
        GateState::Typing
    }
}

/// The number of visual rows the buffer occupies when soft-wrapped to `wrap_w` columns, used by the view
/// to size the input region responsively. A greedy word-wrap count that mirrors the widget's
/// `WordOrGlyph` mode closely. It counts scalar chars, so wide glyphs (CJK, width-2) can make it
/// undercount; this only affects how tall the input box grows (clamped 1..=6) — the widget scrolls
/// internally to keep the cursor visible, so it is never a correctness issue.
pub fn wrapped_line_count(buffer: &InputBuffer, wrap_w: usize) -> usize {
    let wrap_w = wrap_w.max(1);
    let text = buffer.text();
    text.split('\n')
        .map(|logical| logical_rows(logical, wrap_w))
        .sum::<usize>()
        .max(1)
}

/// Rows a single logical line needs: greedy word packing, hard-splitting any word wider than the row.
fn logical_rows(line: &str, w: usize) -> usize {
    if line.is_empty() {
        return 1;
    }
    let mut rows = 0usize;
    let mut col = 0usize; // columns filled on the current open row
    let mut open = false;
    for (i, word) in line.split(' ').enumerate() {
        let wlen = word.chars().count();
        let sep = usize::from(i != 0); // a single space precedes every word but the first
        if open && col + sep + wlen <= w {
            col += sep + wlen;
            continue;
        }
        if wlen <= w {
            rows += 1;
            col = wlen;
        } else {
            rows += wlen.div_ceil(w);
            let rem = wlen % w;
            col = if rem == 0 { w } else { rem };
        }
        open = true;
    }
    rows.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn quench_takes_the_accent_then_clears() {
        let now = Instant::now();
        // At the moment of settle the gate takes the busy cyan, then ramps toward its resting colour.
        assert_eq!(
            quench_fg(Some(now), Some(now), Motion::Full),
            Some(theme::HIGHLIGHT)
        );
        // Reduced motion never quenches.
        assert_eq!(quench_fg(Some(now), Some(now), Motion::Reduced), None);
        // Past the window the gate is back to its resting idle colour (no override).
        let later = now + Duration::from_millis(500);
        assert_eq!(quench_fg(Some(now), Some(later), Motion::Full), None);
        // With no settle recorded there is nothing to quench.
        assert_eq!(quench_fg(None, Some(now), Motion::Full), None);
    }

    fn buffer(text: &str) -> InputBuffer {
        let mut b = InputBuffer::default();
        b.set(text.to_string());
        b
    }

    #[test]
    fn empty_buffer_still_occupies_one_row() {
        assert_eq!(wrapped_line_count(&InputBuffer::default(), 40), 1);
        assert_eq!(wrapped_line_count(&buffer("hello"), 40), 1);
    }

    #[test]
    fn newlines_add_rows() {
        assert_eq!(wrapped_line_count(&buffer("a\nb"), 40), 2);
        assert_eq!(wrapped_line_count(&buffer("a\n"), 40), 2); // trailing blank line still counts
    }

    #[test]
    fn long_word_hard_splits_into_rows() {
        assert_eq!(wrapped_line_count(&buffer("verylongword"), 4), 3); // ceil(12 / 4)
    }

    #[test]
    fn greedy_word_wrap_counts_rows() {
        // "aa bb cc" at width 4 wraps to ["aa", "bb", "cc"] — each pair plus a space overflows.
        assert_eq!(wrapped_line_count(&buffer("aa bb cc"), 4), 3);
    }
}
