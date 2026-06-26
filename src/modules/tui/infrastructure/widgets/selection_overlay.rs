//! In-app screen text selection: paint the highlight over the rendered buffer as a final overlay pass.
//! It works in absolute terminal cells, so it covers the whole UI uniformly — transcript, tool output,
//! and the composer — independent of how each region was rendered. The runtime scrapes the same cells
//! back to text (see `scrape`), so the highlight and the copy always agree.

use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::modules::tui::domain::view_state::{Granularity, ScreenSelection};

/// Paint the selection highlight onto the rendered buffer. Changes only each cell's style (never its
/// symbol), so a later scrape still reads the original glyphs, and reads no clock, so a stable selection
/// keeps the frame byte-identical over time. Out-of-bounds cells are skipped — never a panic.
pub fn paint(buf: &mut Buffer, area: Rect, sel: &ScreenSelection, style: Style) {
    let (start, end) = resolve(buf, sel, area);
    let y0 = start.1.max(area.y);
    let y1 = end.1.min(area.bottom().saturating_sub(1));
    for y in y0..=y1 {
        let (x0, x1) = row_span(start, end, y, area);
        let x0 = x0.max(area.x);
        let x1 = x1.min(area.right().saturating_sub(1));
        for x in x0..=x1 {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(style);
            }
        }
    }
}

/// Scrape the selected text out of the rendered buffer. Reads `Cell::symbol()` left-to-right, advancing
/// by each cell's display width so a wide glyph's blank continuation cell is skipped (never a stray
/// space). Trailing blanks are trimmed per row; rows join with `\n`. Same geometry as `paint`, so what
/// is highlighted is exactly what is copied. Out-of-bounds cells stop the row — never a panic.
pub fn scrape(buf: &Buffer, sel: &ScreenSelection, area: Rect) -> String {
    let (start, end) = resolve(buf, sel, area);
    let y0 = start.1.max(area.y);
    let y1 = end.1.min(area.bottom().saturating_sub(1));
    let mut out = String::new();
    let mut first = true;
    for y in y0..=y1 {
        if !first {
            out.push('\n');
        }
        first = false;
        let (x0, x1) = row_span(start, end, y, area);
        let x0 = x0.max(area.x);
        let x1 = x1.min(area.right().saturating_sub(1));
        let mut line = String::new();
        let mut x = x0;
        while x <= x1 {
            let Some(cell) = buf.cell((x, y)) else { break };
            line.push_str(cell.symbol());
            x = x.saturating_add(cell.cell_width().max(1));
        }
        out.push_str(line.trim_end());
    }
    out
}

/// Resolve a selection to ordered, granularity-expanded, glyph-snapped endpoints (both inclusive) in
/// absolute cells. Shared by `paint` and `scrape` so the highlight and the copied text always match.
pub(super) fn resolve(buf: &Buffer, sel: &ScreenSelection, area: Rect) -> ((u16, u16), (u16, u16)) {
    let (start, end) = sel.ordered();
    let (s, e) = match sel.granularity {
        Granularity::Char => (start, end),
        Granularity::Word => {
            let (ws, _) = word_bounds(buf, start, area);
            let (_, we) = word_bounds(buf, end, area);
            (ws, we)
        }
        Granularity::Line => ((area.x, start.1), (area.right().saturating_sub(1), end.1)),
    };
    (snap_start(buf, s, area), snap_end(buf, e, area))
}

/// The inclusive `(x0, x1)` column span selected on row `y`: the first row runs from the anchor column
/// to the edge, interior rows span the full width, the last row runs from the edge to the head column.
pub(super) fn row_span(start: (u16, u16), end: (u16, u16), y: u16, area: Rect) -> (u16, u16) {
    let left = area.x;
    let right = area.right().saturating_sub(1);
    if start.1 == end.1 {
        (start.0, end.0)
    } else if y == start.1 {
        (start.0, right)
    } else if y == end.1 {
        (left, end.0)
    } else {
        (left, right)
    }
}

/// Whether `(x, y)` is the trailing (blank) half of a wide glyph: its left neighbor is a width-2 cell.
fn is_continuation(buf: &Buffer, x: u16, y: u16, left: u16) -> bool {
    x > left && buf.cell((x - 1, y)).is_some_and(|c| c.cell_width() == 2)
}

/// If the start lands on a wide-glyph continuation cell, snap back to the glyph's lead cell so the whole
/// glyph is selected (and the scrape reads it, never a stray blank).
fn snap_start(buf: &Buffer, pos: (u16, u16), area: Rect) -> (u16, u16) {
    let (x, y) = pos;
    if is_continuation(buf, x, y, area.x) {
        (x - 1, y)
    } else {
        (x, y)
    }
}

/// If the end lands on a wide-glyph lead cell, extend onto its continuation so the highlight covers the
/// whole glyph.
fn snap_end(buf: &Buffer, pos: (u16, u16), area: Rect) -> (u16, u16) {
    let (x, y) = pos;
    let extends = buf.cell((x, y)).is_some_and(|c| c.cell_width() == 2);
    if extends && x + 1 < area.right() {
        (x + 1, y)
    } else {
        (x, y)
    }
}

/// The inclusive cell bounds of the word under `pos` on its row: a maximal run of non-blank cells.
/// Wide-glyph continuation cells (blank, but part of the preceding glyph) are not treated as boundaries,
/// so a CJK run stays one word. A click on a blank cell selects just that cell.
fn word_bounds(buf: &Buffer, pos: (u16, u16), area: Rect) -> ((u16, u16), (u16, u16)) {
    let (x, y) = pos;
    let left = area.x;
    let right = area.right();
    let boundary = |x: u16| -> bool {
        if is_continuation(buf, x, y, left) {
            return false;
        }
        buf.cell((x, y))
            .is_none_or(|c| c.symbol().trim().is_empty())
    };
    if boundary(x) {
        return ((x, y), (x, y));
    }
    let mut sx = x;
    while sx > left && !boundary(sx - 1) {
        sx -= 1;
    }
    let mut ex = x;
    while ex + 1 < right && !boundary(ex + 1) {
        ex += 1;
    }
    ((sx, y), (ex, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::infrastructure::theme;

    fn buf(lines: &[&str]) -> Buffer {
        Buffer::with_lines(lines.iter().copied())
    }

    fn char_sel(anchor: (u16, u16), head: (u16, u16)) -> ScreenSelection {
        let mut s = ScreenSelection::new(anchor.0, anchor.1, Granularity::Char);
        s.extend(head.0, head.1);
        s
    }

    #[test]
    fn paint_highlights_only_the_selected_cells() {
        let mut b = buf(&["abcdef"]);
        let area = b.area;
        paint(&mut b, area, &char_sel((1, 0), (3, 0)), theme::selection());
        for x in [1u16, 2, 3] {
            assert_eq!(b[(x, 0)].style().bg, Some(theme::BRAND), "col {x} selected");
        }
        for x in [0u16, 4, 5] {
            assert_ne!(
                b[(x, 0)].style().bg,
                Some(theme::BRAND),
                "col {x} untouched"
            );
        }
    }

    #[test]
    fn paint_leaves_symbols_unchanged() {
        let mut b = buf(&["abcdef"]);
        let before: Vec<String> = (0..6).map(|x| b[(x, 0)].symbol().to_string()).collect();
        let area = b.area;
        paint(&mut b, area, &char_sel((0, 0), (5, 0)), theme::selection());
        let after: Vec<String> = (0..6).map(|x| b[(x, 0)].symbol().to_string()).collect();
        assert_eq!(
            before, after,
            "paint must not touch symbols (scrape relies on it)"
        );
    }

    #[test]
    fn paint_highlights_both_cells_of_a_wide_glyph() {
        // "世" occupies cols 0-1 (lead + blank continuation); a drag onto the continuation highlights both.
        let mut b = buf(&["世a"]);
        let area = b.area;
        paint(&mut b, area, &char_sel((0, 0), (1, 0)), theme::selection());
        assert_eq!(b[(0, 0)].style().bg, Some(theme::BRAND));
        assert_eq!(b[(1, 0)].style().bg, Some(theme::BRAND));
        assert_eq!(
            b[(0, 0)].symbol(),
            "世",
            "the glyph symbol stays in its lead cell"
        );
    }

    #[test]
    fn paint_word_granularity_covers_a_cjk_run() {
        // A double-click (Word) on the first glyph of "世界" selects the whole run, blanks-between-glyphs
        // (continuations) included — both lead cells get the highlight.
        let mut b = buf(&["世界 x"]);
        let area = b.area;
        let sel = ScreenSelection::new(0, 0, Granularity::Word);
        paint(&mut b, area, &sel, theme::selection());
        assert_eq!(b[(0, 0)].style().bg, Some(theme::BRAND), "世 lead");
        assert_eq!(b[(2, 0)].style().bg, Some(theme::BRAND), "界 lead");
        // The space after the run is a boundary — not selected.
        assert_ne!(
            b[(4, 0)].style().bg,
            Some(theme::BRAND),
            "space after the word"
        );
    }

    #[test]
    fn paint_is_panic_safe_with_out_of_bounds_selection() {
        // Stale coords (e.g. a pre-resize selection) must clamp to the buffer, never panic.
        let mut b = buf(&["abc", "def"]); // 3x2
        let area = b.area;
        paint(
            &mut b,
            area,
            &char_sel((0, 0), (40, 40)),
            theme::selection(),
        );
        // Reached here without panicking; the in-bounds cells are still valid.
        assert_eq!(b[(0, 0)].style().bg, Some(theme::BRAND));
    }

    #[test]
    fn scrape_single_row_right_trims() {
        let b = buf(&["hi   "]);
        assert_eq!(scrape(&b, &char_sel((0, 0), (4, 0)), b.area), "hi");
    }

    #[test]
    fn scrape_multi_row_joins_with_newline() {
        let b = buf(&["ab", "cd"]);
        assert_eq!(scrape(&b, &char_sel((0, 0), (1, 1)), b.area), "ab\ncd");
    }

    #[test]
    fn scrape_partial_columns_is_a_substring() {
        let b = buf(&["abcdef"]);
        assert_eq!(scrape(&b, &char_sel((1, 0), (3, 0)), b.area), "bcd");
    }

    #[test]
    fn scrape_wide_glyph_no_duplication() {
        // The continuation cell of a wide glyph reads as a space; advancing by cell width skips it, so a
        // CJK run is copied verbatim with no spurious spaces.
        let b = buf(&["世界"]);
        assert_eq!(scrape(&b, &char_sel((0, 0), (3, 0)), b.area), "世界");
    }

    #[test]
    fn scrape_boundary_on_wide_continuation_includes_whole_glyph() {
        // A selection that lands on the blank continuation half of "世" still copies the whole glyph.
        let b = buf(&["世界"]);
        assert_eq!(scrape(&b, &char_sel((1, 0), (1, 0)), b.area), "世");
    }

    #[test]
    fn scrape_preserves_a_blank_middle_row() {
        let b = buf(&["xy", "  ", "zw"]);
        assert_eq!(scrape(&b, &char_sel((0, 0), (1, 2)), b.area), "xy\n\nzw");
    }

    #[test]
    fn scrape_word_selection_returns_the_word() {
        let b = buf(&["foo bar"]);
        let sel = ScreenSelection::new(0, 0, Granularity::Word);
        assert_eq!(scrape(&b, &sel, b.area), "foo");
    }

    #[test]
    fn scrape_line_selection_returns_the_whole_row() {
        let b = buf(&["foo bar"]);
        let sel = ScreenSelection::new(2, 0, Granularity::Line);
        assert_eq!(scrape(&b, &sel, b.area), "foo bar");
    }

    #[test]
    fn scrape_is_panic_safe_and_bounded_with_out_of_bounds_selection() {
        let b = buf(&["abc", "def"]); // 3x2
        // A whole-screen selection is bounded by the buffer height; stale coords never panic.
        let text = scrape(&b, &char_sel((0, 0), (40, 40)), b.area);
        assert_eq!(text.lines().count(), 2);
        assert_eq!(text, "abc\ndef");
    }
}
