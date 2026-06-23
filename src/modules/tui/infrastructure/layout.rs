use ratatui::layout::{Constraint, Layout, Rect};

/// The stacked regions of the core. The brand seal sits on top; the model/workspace context drops
/// down to the forged `meta` rule directly above the input, so identity and context cluster around where
/// the user actually types. `thinking` is the optional live-reasoning line shown while a turn runs.
pub struct Regions {
    pub header: Rect,
    pub transcript: Rect,
    pub meta: Rect,
    pub thinking: Rect,
    pub input: Rect,
    pub hint: Rect,
}

/// Split the frame into the brand seal, the transcript, the forged meta rule, the optional live-thinking
/// line, the borderless input editor (height grows with the buffer, capped), and the hint line. The
/// header and hint collapse to zero height on very short terminals so the transcript always keeps at
/// least one row; the thinking line only appears when `show_thinking` and there is room for it.
pub fn frame_layout(area: Rect, input_lines: u16, show_thinking: bool) -> Regions {
    let input_height = input_lines.clamp(1, 6); // borderless: no extra rows for a frame
    let short = area.height < 8;
    let header_h = if short { 0 } else { 1 };
    let hint_h = if short { 0 } else { 1 };
    let thinking_h = match show_thinking {
        true if area.height >= 10 => 1,
        _ => 0,
    };
    let [header, transcript, meta, thinking, input, hint] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(thinking_h),
        Constraint::Length(input_height),
        Constraint::Length(hint_h),
    ])
    .areas(area);
    Regions {
        header,
        transcript,
        meta,
        thinking,
        input,
        hint,
    }
}
