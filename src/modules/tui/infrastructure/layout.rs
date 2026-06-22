use ratatui::layout::{Constraint, Layout, Rect};

/// The five stacked regions of the core. The brand seal sits on top; the model/workspace context drops
/// down to the forged `meta` rule directly above the input, so identity and context cluster around where
/// the user actually types.
pub struct Regions {
    pub header: Rect,
    pub transcript: Rect,
    pub meta: Rect,
    pub input: Rect,
    pub hint: Rect,
}

/// Split the frame into the brand seal, the transcript, the forged meta rule, the borderless input
/// editor (height grows with the buffer, capped), and the hint line. `input_lines` is the editor's
/// logical line count.
pub fn frame_layout(area: Rect, input_lines: u16) -> Regions {
    let input_height = input_lines.clamp(1, 6); // borderless: no extra rows for a frame
    let [header, transcript, meta, input, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);
    Regions {
        header,
        transcript,
        meta,
        input,
        hint,
    }
}
