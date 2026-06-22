use ratatui::layout::{Constraint, Layout, Rect};

/// The four stacked regions of the single-pane core. Panels (a later phase) split `transcript`
/// further; this base layout is complete on its own.
pub struct Regions {
    pub status: Rect,
    pub transcript: Rect,
    pub input: Rect,
    pub hint: Rect,
}

/// Split the frame into status bar, transcript, input editor (height grows with the buffer, capped),
/// and the hint line. `input_lines` is the editor's logical line count.
pub fn frame_layout(area: Rect, input_lines: u16) -> Regions {
    let input_height = input_lines.clamp(1, 6) + 2; // +2 for the editor's border
    let [status, transcript, input, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);
    Regions {
        status,
        transcript,
        input,
        hint,
    }
}
