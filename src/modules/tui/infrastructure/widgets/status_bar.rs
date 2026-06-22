use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// Render the top status line: the brand, the model id, the workspace, and — while a turn runs — a
/// spinner with the elapsed seconds.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let mut spans = vec![
        Span::styled(" kiri ", theme::accent()),
        Span::raw("  "),
        Span::raw(model.status.model.clone()),
        Span::raw("  ·  "),
        Span::raw(model.status.workspace.clone()),
    ];
    if model.busy {
        let glyph = theme::SPINNER[model.status.spinner_frame % theme::SPINNER.len()];
        spans.push(Span::raw("  ·  "));
        spans.push(Span::styled(
            format!("{glyph} {}s", model.status.elapsed_secs),
            theme::dim(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
