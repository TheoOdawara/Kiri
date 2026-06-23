use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::theme;

/// The slim top brand line: the KIRI seal and the tagline. The model, workspace, and turn indicators now
/// live on the forged meta rule just above the input (see `meta_rule`). On narrow terminals the tagline
/// is dropped so the seal never overflows.
pub fn render(_model: &Model, frame: &mut Frame, area: Rect) {
    let seal = " ⬢ kiri ";
    let tagline = " Engineering-Grade Code Harness";
    let full = format!("{seal}{tagline}");
    let line = if full.chars().count() <= area.width as usize {
        Line::from(vec![
            Span::styled(seal, theme::dim().add_modifier(Modifier::BOLD)),
            Span::styled(tagline, theme::dim()),
        ])
    } else {
        Line::styled(seal, theme::dim().add_modifier(Modifier::BOLD))
    };
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
