use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// The slim top brand line: the KIRI seal and the tagline. The model, workspace, and turn indicators now
/// live on the forged meta rule just above the input (see `meta_rule`).
pub fn render(_model: &Model, frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" ⬢ kiri ", theme::dim().add_modifier(Modifier::BOLD)),
        Span::styled(" Engineering-Grade Code Harness", theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
