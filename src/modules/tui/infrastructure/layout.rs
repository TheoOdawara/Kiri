use ratatui::layout::{Constraint, Layout, Rect};

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

/// Split the frame into the brand seal, the transcript, the forged meta rule, the optional confirmation
/// box, the borderless input editor (height grows with the buffer, capped), and the hint line. The
/// header and hint collapse to zero height on very short terminals so the transcript always keeps at
/// least one row; `box_h` is the height the caller computed for a pending plan/approval box (0 when none).
pub fn frame_layout(area: Rect, input_lines: u16, box_h: u16) -> Regions {
    let input_height = input_lines.clamp(1, 6); // borderless: no extra rows for a frame
    let short = area.height < 8;
    let header_h = if short { 0 } else { 1 };
    let hint_h = if short { 0 } else { 1 };
    // The confirmation box must render in full (the user reads its options to answer), so it takes
    // priority over the transcript: cap it to the rows left by the fixed chrome (header, meta, input,
    // hint) and let the transcript yield to zero while a box is up. With no box, the transcript keeps
    // at least one row.
    let chrome = header_h + 1 + input_height + hint_h; // meta is always one row
    let box_h = box_h.min(area.height.saturating_sub(chrome));
    let transcript_min = if box_h > 0 { 0 } else { 1 };
    let [header, transcript, meta, prompt_box, input, hint] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(transcript_min),
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
