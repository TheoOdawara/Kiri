use ratatui::layout::{Constraint, Layout, Margin, Rect};

/// Columns of side gutter (per edge) reserved on a roomy terminal so content never touches the edges.
/// Generous (asymmetric-feeling) so the column floats off the wall — part of killing the boxed look.
const SIDE_GUTTER: u16 = 6;
/// Minimum width/height for the terminal to be considered "roomy" enough for decorative padding.
const MIN_ROOMY_WIDTH: u16 = 60;
const MIN_ROOMY_HEIGHT: u16 = 12;
/// Bounds for the borderless input editor height: at least one row, capped so it never eats the frame.
const MIN_INPUT_HEIGHT: u16 = 1;
const MAX_INPUT_HEIGHT: u16 = 6;
/// Below this height the terminal is "short": header and hint collapse so the transcript keeps a row.
const SHORT_TERMINAL_HEIGHT: u16 = 8;

/// The stacked regions of the core. The brand seal sits on top; the model/workspace context drops
/// down to the forged `meta` rule directly above the input, so identity and context cluster around where
/// the user actually types. `prompt_box` is the dedicated slot — directly above the input — for a pending
/// plan or tool-call confirmation, so the box is always anchored to the bottom, never lost in the
/// scrolling transcript.
pub struct Regions {
    pub header: Rect,
    pub transcript: Rect,
    pub meta: Rect,
    pub prompt_box: Rect,
    pub input: Rect,
    pub hint: Rect,
}

/// Horizontal gutter (columns per side) reserved around the whole UI so content never touches the
/// terminal edges. Generous when the terminal is roomy, zero on small ones so nothing is squeezed.
/// Public so the view sizes the input wrap width against the same content width.
pub fn h_pad(area: Rect) -> u16 {
    if roomy(area) { SIDE_GUTTER } else { 0 }
}

/// Whether the terminal has room to spare for decorative padding (side gutters, top margin, the gap
/// above the input cluster). Below this the UI runs edge-to-edge to keep every row usable.
fn roomy(area: Rect) -> bool {
    area.width >= MIN_ROOMY_WIDTH && area.height >= MIN_ROOMY_HEIGHT
}

/// Split the frame into the brand seal, the transcript, the forged meta rule, the optional confirmation
/// box, the borderless input editor (height grows with the buffer, capped), and the hint line. The
/// header and hint collapse to zero height on very short terminals so the transcript always keeps at
/// least one row; `box_h` is the height the caller computed for a pending plan/approval box (0 when none).
/// On a roomy terminal the whole stack is inset by a gutter (sides + top/bottom) and a gap separates the
/// scrolling transcript from the input cluster; both collapse to zero on small terminals.
pub fn frame_layout(area: Rect, input_lines: u16, box_h: u16) -> Regions {
    let roomy = roomy(area);
    // A pending plan/approval box must render in full, so when one is up the decorative vertical
    // padding and the gap yield their rows to it; the side gutters (which cost no rows) always stay.
    let has_box = box_h > 0;
    let vertical_pad = if roomy && !has_box { 1 } else { 0 };
    let gap_h = if roomy && !has_box { 2 } else { 0 };
    // Inset the stack so content breathes away from the edges. The base block already paints the full
    // frame, so the margin shows as background padding rather than a gap artifact.
    let area = area.inner(Margin {
        horizontal: if roomy { SIDE_GUTTER } else { 0 },
        vertical: vertical_pad,
    });

    let input_height = input_lines.clamp(MIN_INPUT_HEIGHT, MAX_INPUT_HEIGHT); // borderless: no frame rows
    let short = area.height < SHORT_TERMINAL_HEIGHT;
    let header_h = if short { 0 } else { 1 };
    let hint_h = if short { 0 } else { 1 };
    // The confirmation box must render in full (the user reads its options to answer), so it takes
    // priority over the transcript: cap it to the rows left by the fixed chrome (header, gap, meta,
    // input, hint) and let the transcript yield to zero while a box is up. With no box, the transcript
    // keeps at least one row.
    let chrome = header_h + gap_h + 1 + input_height + hint_h; // meta is always one row
    let box_h = box_h.min(area.height.saturating_sub(chrome));
    let transcript_min = if box_h > 0 { 0 } else { 1 };
    let [header, transcript, _gap, meta, prompt_box, input, hint] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(transcript_min),
        Constraint::Length(gap_h),
        Constraint::Length(1),
        Constraint::Length(box_h),
        Constraint::Length(input_height),
        Constraint::Length(hint_h),
    ])
    .areas(area);
    Regions {
        header,
        transcript,
        meta,
        prompt_box,
        input,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roomy_terminal_gets_gutters_top_margin_and_a_gap() {
        let r = frame_layout(Rect::new(0, 0, 100, 30), 1, 0);
        assert_eq!(r.header.x, 6, "left gutter expected");
        assert_eq!(r.header.width, 88, "side gutters of 6 each expected");
        assert_eq!(r.header.y, 1, "top margin expected");
        // A two-row gap separates the transcript from the meta rule (the input cluster).
        let transcript_end = r.transcript.y + r.transcript.height;
        assert_eq!(
            r.meta.y - transcript_end,
            2,
            "gap above the meta rule expected"
        );
    }

    #[test]
    fn small_terminal_runs_edge_to_edge() {
        // Below the roominess threshold (width < 60): no gutter, no top margin, no gap.
        let r = frame_layout(Rect::new(0, 0, 50, 20), 1, 0);
        assert_eq!(r.header.x, 0, "no gutter on a small terminal");
        assert_eq!(r.header.width, 50, "full width on a small terminal");
        assert_eq!(r.header.y, 0, "no top margin on a small terminal");
        let transcript_end = r.transcript.y + r.transcript.height;
        assert_eq!(r.meta.y, transcript_end, "no gap on a small terminal");
    }

    #[test]
    fn h_pad_is_zero_when_not_roomy() {
        assert_eq!(h_pad(Rect::new(0, 0, 50, 20)), 0);
        assert_eq!(h_pad(Rect::new(0, 0, 100, 10)), 0);
        assert_eq!(h_pad(Rect::new(0, 0, 100, 30)), 6);
    }
}
