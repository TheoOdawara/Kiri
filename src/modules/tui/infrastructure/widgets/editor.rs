use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use unicode_width::UnicodeWidthChar;

use crate::modules::tui::domain::input_buffer::InputBuffer;
use crate::modules::tui::domain::model::{Model, Motion};
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::infrastructure::text::display_width;
use crate::modules::tui::infrastructure::theme::{self, GateState};

/// How long the gate's temper quench lasts after a turn settles.
const QUENCH_MS: f32 = 400.0;
/// The living cursor's full pulse period (dim → accent → dim).
const PULSE_MS: u128 = 1280;

/// Display columns of the prompt prefix `⬡ ›▏ ` (gate glyph + space + `›` + cursor bar + space). The gate
/// glyph and `›` are width-1 in the terminals we target, so the text column stays aligned. Public so the
/// view can count wrapped input lines against the same column budget.
pub const PROMPT_COLS: u16 = 5;

/// Render the borderless input editor. A left gutter carries the state-driven gate seal and the `›▏`
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
    // The `_` placeholder becomes the living cursor: a thin bar that pulses between dim and the accent —
    // the one sanctioned idle motion, a banked coal that says the harness is awake. The hexagon gate to
    // its left stays perfectly still.
    let cursor_style = Style::default().fg(cursor_fg(model.opened_at, model.render_at, motion));
    let prompt = Line::from(vec![
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled("›", theme::dim()),
        Span::styled("▏", cursor_style),
        Span::styled(" ", theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(prompt).style(theme::base()), gutter);

    let editor_area = content_rect(area);
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

/// The living cursor's colour: a steady dim bar under reduced motion, otherwise a triangle pulse between
/// dim steel and the accent over `PULSE_MS`, phased off the open instant. Pure, so the one idle motion is
/// unit-testable; it is also the single exception to the idle zero-diff rule.
fn cursor_fg(opened_at: Option<Instant>, now: Option<Instant>, motion: Motion) -> Color {
    if motion.is_reduced() {
        return theme::BRAND;
    }
    let phase = match (opened_at, now) {
        (Some(o), Some(n)) => (n.saturating_duration_since(o).as_millis() % PULSE_MS) as f32,
        _ => return theme::BRAND,
    };
    let t = phase / PULSE_MS as f32; // 0..1 across the period
    let triangle = 1.0 - (2.0 * t - 1.0).abs(); // 0 → 1 → 0
    theme::ramp(&[theme::BRAND, theme::HIGHLIGHT], triangle)
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

/// The editor's text sub-rect inside the input region: the prompt gutter (`PROMPT_COLS`) is carved off
/// the left, exactly as `render` draws the `tui-textarea` widget. Shared so a mouse click can be mapped
/// against the same cells the widget rendered into.
pub fn content_rect(input_area: Rect) -> Rect {
    let gutter = PROMPT_COLS.min(input_area.width);
    Rect {
        x: input_area.x + gutter,
        width: input_area.width.saturating_sub(gutter),
        ..input_area
    }
}

/// Map a click at absolute screen `(col, row)` to a logical `(row, col)` cursor position in `buffer`,
/// but only where the rendered layout is unambiguous: every logical line fits `editor`'s width (no
/// soft-wrap, so screen rows equal logical rows) and they all fit its height (no internal scroll we
/// cannot read). Returns `None` outside the text rect or when the layout is ambiguous — the caller then
/// leaves the cursor put rather than mis-place it, because `tui-textarea` exposes no public
/// screen→cursor mapping to reconcile its wrap/scroll against.
pub fn click_to_cursor(
    buffer: &InputBuffer,
    editor: Rect,
    col: u16,
    row: u16,
) -> Option<(usize, usize)> {
    if col < editor.x || col >= editor.right() || row < editor.y || row >= editor.bottom() {
        return None;
    }
    let wrap_w = editor.width as usize;
    let text = buffer.text();
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.iter().any(|line| display_width(line) > wrap_w) {
        return None; // a line soft-wraps → screen rows no longer match logical rows
    }
    if lines.len() > editor.height as usize {
        return None; // content taller than the box → internal vertical scroll we cannot read
    }
    let logical_row = ((row - editor.y) as usize).min(lines.len().saturating_sub(1));
    let target = (col - editor.x) as usize;
    let logical_col = col_at_width(lines[logical_row], target);
    Some((logical_row, logical_col))
}

/// The char index in `line` at the greatest cell boundary `<= target`. Walks by display width so a
/// click lands on a glyph boundary (never inside a wide glyph); a click past the line end clamps to it.
fn col_at_width(line: &str, target: usize) -> usize {
    let mut acc = 0usize;
    for (i, ch) in line.chars().enumerate() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > target {
            return i;
        }
        acc += w;
    }
    line.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cursor_is_steady_dim_when_frozen_or_at_rest() {
        let now = Instant::now();
        // Reduced motion holds a steady dim bar.
        assert_eq!(
            cursor_fg(Some(now), Some(now), Motion::Reduced),
            theme::BRAND
        );
        // No clock yet → steady dim.
        assert_eq!(cursor_fg(None, None, Motion::Full), theme::BRAND);
        // The pulse starts at the dim end of the ramp.
        assert_eq!(cursor_fg(Some(now), Some(now), Motion::Full), theme::BRAND);
    }

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

    #[test]
    fn content_rect_carves_the_prompt_gutter() {
        let editor = content_rect(Rect::new(0, 7, 40, 1));
        assert_eq!(editor.x, PROMPT_COLS);
        assert_eq!(editor.width, 40 - PROMPT_COLS);
        assert_eq!((editor.y, editor.height), (7, 1));
    }

    #[test]
    fn content_rect_clamps_when_narrower_than_gutter() {
        // The gutter never exceeds the region width; the text rect then has zero width, not an underflow.
        let editor = content_rect(Rect::new(2, 0, 3, 1));
        assert_eq!((editor.x, editor.width), (5, 0));
    }

    /// A canonical editor text rect: left at column 5 (past the gutter), row 10, 30 wide, 3 tall.
    fn editor_rect() -> Rect {
        Rect::new(5, 10, 30, 3)
    }

    #[test]
    fn click_to_cursor_maps_a_simple_single_line() {
        let b = buffer("hello");
        // Two cells into the text on the first row → logical (0, 2).
        assert_eq!(click_to_cursor(&b, editor_rect(), 5 + 2, 10), Some((0, 2)));
    }

    #[test]
    fn click_to_cursor_maps_multiline_rows() {
        let b = buffer("ab\ncd\nef");
        assert_eq!(click_to_cursor(&b, editor_rect(), 5 + 1, 12), Some((2, 1)));
    }

    #[test]
    fn click_to_cursor_walks_display_width_for_wide_glyphs() {
        // "世a": 世 spans cols 0-1, 'a' is at col 2. A click on the 'a' resolves to char index 1.
        let b = buffer("世a");
        assert_eq!(click_to_cursor(&b, editor_rect(), 5 + 2, 10), Some((0, 1)));
        // A click inside 世 (its second cell) snaps back to its boundary — char index 0.
        assert_eq!(click_to_cursor(&b, editor_rect(), 5 + 1, 10), Some((0, 0)));
    }

    #[test]
    fn click_to_cursor_clamps_past_line_end() {
        let b = buffer("hi");
        // Far to the right but still inside the rect → clamps to the line end (char index 2).
        assert_eq!(click_to_cursor(&b, editor_rect(), 5 + 20, 10), Some((0, 2)));
    }

    #[test]
    fn click_to_cursor_returns_none_in_the_gutter() {
        let b = buffer("hi");
        // A column left of the text rect (in the prompt gutter) is not editor text.
        assert_eq!(click_to_cursor(&b, editor_rect(), 4, 10), None);
    }

    #[test]
    fn click_to_cursor_returns_none_outside_right_edge() {
        let b = buffer("hi");
        assert_eq!(click_to_cursor(&b, editor_rect(), 5 + 30, 10), None); // == right(), exclusive
    }

    #[test]
    fn click_below_the_box_returns_none() {
        let b = buffer("hi");
        assert_eq!(click_to_cursor(&b, editor_rect(), 5, 13), None); // == bottom(), exclusive
    }

    #[test]
    fn click_below_text_but_inside_box_clamps_to_last_row() {
        // One line of text in a 3-tall box: a click on the empty third row clamps to the last logical row.
        let b = buffer("hi");
        assert_eq!(click_to_cursor(&b, editor_rect(), 5 + 1, 12), Some((0, 1)));
    }

    #[test]
    fn click_to_cursor_returns_none_when_a_line_wraps() {
        // A 5-wide rect with a 6-char line: the widget soft-wraps it, so screen rows lie about logic.
        let narrow = Rect::new(5, 10, 5, 3);
        assert_eq!(click_to_cursor(&buffer("abcdef"), narrow, 6, 10), None);
    }

    #[test]
    fn click_to_cursor_returns_none_when_scrolled() {
        // Three lines in a 2-tall box: the widget scrolls internally, an offset we cannot read.
        let short = Rect::new(5, 10, 30, 2);
        assert_eq!(click_to_cursor(&buffer("a\nb\nc"), short, 5, 10), None);
    }
}
