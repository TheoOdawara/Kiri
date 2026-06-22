use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::infrastructure::theme::{self, GateState};

/// Display columns of the prompt prefix `⬡ ›_ ` (gate glyph + space + `›` + `_` + space). The gate glyph
/// and `›` are width-1 in the terminals we target, so the cursor math stays exact.
const PROMPT_COLS: u16 = 5;

/// Render the borderless input editor. The first line carries the state-driven gate seal and the `›_`
/// prompt; wrapped/continuation lines indent by `PROMPT_COLS` so the text column stays aligned. The
/// hardware cursor is placed only when the editor is active (no pending approval, not mid-turn).
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let active = model.pending_approval.is_none() && !model.busy;
    let (glyph, glyph_style) = theme::gate(gate_state(model));
    let text = model.input.text();

    let mut lines: Vec<Line> = Vec::new();
    for (i, logical) in text.split('\n').enumerate() {
        let mut spans = if i == 0 {
            vec![
                Span::styled(format!("{glyph} "), glyph_style),
                Span::styled("›_ ", theme::dim()),
            ]
        } else {
            vec![Span::raw(" ".repeat(PROMPT_COLS as usize))]
        };
        spans.push(Span::styled(logical.to_string(), theme::base()));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines).style(theme::base()), area);

    if active {
        let (col, row) = cursor_position(text, model.input.cursor());
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

/// Column and row of the cursor within the buffer (rows separated by newlines; no soft-wrap accounted).
fn cursor_position(text: &str, cursor: usize) -> (u16, u16) {
    let before = &text[..cursor];
    let row = before.matches('\n').count() as u16;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() as u16;
    (col, row)
}
