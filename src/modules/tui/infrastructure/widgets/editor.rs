use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::domain::view_state::InputBuffer;
use crate::modules::tui::infrastructure::theme::{self, GateState};

/// Display columns of the prompt prefix `⬡ ›_ ` (gate glyph + space + `›` + `_` + space). The gate glyph
/// and `›` are width-1 in the terminals we target, so the cursor math stays exact. Public so the view
/// can count wrapped input lines against the same column budget.
pub const PROMPT_COLS: u16 = 5;

/// Render the borderless input editor. The first visual line carries the state-driven gate seal and the
/// `›_` prompt; wrapped continuation lines indent by `PROMPT_COLS` so the text column stays aligned.
/// Long lines soft-wrap at word boundaries (a word wider than the width is hard-cut so it never
/// overflows). The hardware cursor is placed only when the editor is active.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let active = model.pending_approval.is_none() && !model.busy;
    let (glyph, glyph_style) = theme::gate(gate_state(model));
    let text = model.input.text();
    let wrap_w = area.width.saturating_sub(PROMPT_COLS).max(1) as usize;

    let rows = wrap_rows(text, wrap_w);
    let mut lines: Vec<Line> = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let mut spans = if i == 0 {
            vec![
                Span::styled(format!("{glyph} "), glyph_style),
                Span::styled("›_ ", theme::dim()),
            ]
        } else {
            vec![Span::raw(" ".repeat(PROMPT_COLS as usize))]
        };
        spans.push(Span::styled(row.text.clone(), theme::base()));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines).style(theme::base()), area);

    if active {
        let (col, row) = cursor_position(text, model.input.cursor(), wrap_w);
        let max_col = area.width.saturating_sub(PROMPT_COLS + 1);
        frame.set_cursor_position(Position {
            x: area.x + PROMPT_COLS + col.min(max_col),
            y: area.y + row.min(area.height.saturating_sub(1)),
        });
    }
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

/// One visual row of the wrapped input: its text and the byte offset in the buffer where it begins.
/// Both `render` and `cursor_position` consume these, so the drawn layout and the cursor mapping can
/// never disagree on where a row starts.
struct VisualRow {
    text: String,
    start: usize,
}

/// Soft-wrap the buffer to `wrap_w` columns by word, tracking each visual row's byte offset in `text`.
/// A newline starts a new row (its byte is consumed); a word longer than the width is hard-cut into
/// adjacent char chunks (no separator). Empty input yields a single empty row so the prompt renders.
fn wrap_rows(text: &str, wrap_w: usize) -> Vec<VisualRow> {
    let wrap_w = wrap_w.max(1);
    let mut rows: Vec<VisualRow> = Vec::new();
    let mut offset = 0usize; // byte offset in `text` of the start of the current logical line
    for logical in text.split('\n') {
        wrap_logical(logical, offset, wrap_w, &mut rows);
        offset += logical.len() + 1; // advance past the logical line and its trailing '\n'
    }
    if rows.is_empty() {
        rows.push(VisualRow {
            text: String::new(),
            start: 0,
        });
    }
    rows
}

/// Greedy word-wrap one logical line beginning at byte `base` in the buffer, appending `VisualRow`s.
/// An empty line yields one empty row at `base` so blank lines (and the prompt) still render.
fn wrap_logical(line: &str, base: usize, wrap_w: usize, rows: &mut Vec<VisualRow>) {
    if line.is_empty() {
        rows.push(VisualRow {
            text: String::new(),
            start: base,
        });
        return;
    }
    let mut cur = String::new();
    let mut cur_start = base;
    let mut have_cur = false;
    let mut word_start = base; // byte offset of the current word within the buffer
    for word in line.split(' ') {
        let wlen = word.chars().count();
        if !have_cur {
            if wlen <= wrap_w {
                cur = word.to_string();
                cur_start = word_start;
                have_cur = true;
            } else {
                push_hard_cut(word, word_start, wrap_w, rows);
            }
        } else if cur.chars().count() + 1 + wlen <= wrap_w {
            cur.push(' ');
            cur.push_str(word);
        } else {
            rows.push(VisualRow {
                text: std::mem::take(&mut cur),
                start: cur_start,
            });
            have_cur = false;
            if wlen <= wrap_w {
                cur = word.to_string();
                cur_start = word_start;
                have_cur = true;
            } else {
                push_hard_cut(word, word_start, wrap_w, rows);
            }
        }
        word_start += word.len() + 1; // +1 for the single space that `split(' ')` removed
    }
    if have_cur {
        rows.push(VisualRow {
            text: cur,
            start: cur_start,
        });
    }
}

/// Hard-cut a word wider than `wrap_w` into adjacent char chunks, each a `VisualRow` whose `start`
/// tracks its byte offset (chunks are contiguous — there is no separator byte between them).
fn push_hard_cut(word: &str, base: usize, wrap_w: usize, rows: &mut Vec<VisualRow>) {
    let mut byte = base;
    let mut chars = word.chars().peekable();
    while chars.peek().is_some() {
        let chunk: String = chars.by_ref().take(wrap_w).collect();
        let len = chunk.len();
        rows.push(VisualRow {
            text: chunk,
            start: byte,
        });
        byte += len;
    }
}

/// Column (in display chars) and row of the cursor within the wrapped layout, excluding the prompt
/// prefix the caller adds back. Maps the buffer's byte cursor onto the same `VisualRow`s that `render`
/// draws: the column counts chars (so multibyte input stays aligned), and a wrap boundary that consumed
/// a space lands the cursor at the start of the next row.
fn cursor_position(text: &str, cursor: usize, wrap_w: usize) -> (u16, u16) {
    let rows = wrap_rows(text, wrap_w.max(1));
    for (r, row) in rows.iter().enumerate() {
        let end = row.start + row.text.len();
        if cursor < end {
            let col = text
                .get(row.start..cursor)
                .map_or(row.text.chars().count(), |s| s.chars().count());
            return (col as u16, r as u16);
        }
        if cursor == end {
            // At a row boundary: if the next row is contiguous (a hard-cut, no consumed space) the
            // cursor belongs at the start of that next row; otherwise it ends this one.
            let next_is_contiguous = rows.get(r + 1).is_some_and(|n| n.start == end);
            if !next_is_contiguous {
                return (row.text.chars().count() as u16, r as u16);
            }
        }
    }
    let last = rows.len().saturating_sub(1);
    (rows[last].text.chars().count() as u16, last as u16)
}

/// The number of visual lines the input occupies when wrapped to `wrap_w` columns, used by the view to
/// size the input region responsively.
pub fn wrapped_line_count(buffer: &InputBuffer, wrap_w: usize) -> usize {
    wrap_rows(buffer.text(), wrap_w.max(1)).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_column_counts_chars_not_bytes_for_accented_input() {
        // "ação" is 6 bytes / 4 chars: the cursor at the end is column 4, not 6.
        assert_eq!(cursor_position("ação", "ação".len(), 40), (4, 0));
        // Mid-line accents stay aligned too ("café " is 6 bytes / 5 chars).
        assert_eq!(cursor_position("café bar", "café ".len(), 40), (5, 0));
    }

    #[test]
    fn cursor_maps_into_hard_cut_long_word_rows() {
        // "verylongword" at width 4 wraps to ["very", "long", "word"] with no separators.
        assert_eq!(cursor_position("verylongword", 8, 4), (0, 2)); // start of "word"
        assert_eq!(cursor_position("verylongword", 12, 4), (4, 2)); // end of "word"
    }

    #[test]
    fn cursor_after_a_word_wrap_lands_on_the_next_row() {
        // "aa bb" at width 2 wraps to ["aa", "bb"]; the consumed space sits between them.
        assert_eq!(cursor_position("aa bb", 2, 2), (2, 0)); // end of "aa", before the space
        assert_eq!(cursor_position("aa bb", 3, 2), (0, 1)); // start of "bb", after the space
    }

    #[test]
    fn cursor_handles_newlines_and_trailing_blank_line() {
        assert_eq!(cursor_position("ab\ncd", 0, 40), (0, 0));
        assert_eq!(cursor_position("ab\ncd", 3, 40), (0, 1)); // start of the second line
        assert_eq!(cursor_position("ab\n", 3, 40), (0, 1)); // fresh empty line after the newline
    }

    #[test]
    fn wrapped_rows_count_matches_layout() {
        assert_eq!(wrap_rows("", 40).len(), 1); // empty still renders the prompt row
        assert_eq!(wrap_rows("hello", 40).len(), 1);
        assert_eq!(wrap_rows("a\nb", 40).len(), 2);
        assert_eq!(wrap_rows("verylongword", 4).len(), 3); // ceil(12 / 4)
    }
}
