use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::text::display_width;
use crate::modules::tui::infrastructure::theme;

/// The slim top brand line: the KIRI seal and the tagline. The model, workspace, and turn indicators now
/// live on the forged meta rule just above the input (see `meta_rule`). On narrow terminals the tagline
/// is dropped so the seal never overflows. While a confirmation is up the seal recedes one further ramp
/// step, sinking behind the decision.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let seal = " ⬢ kiri ";
    let tagline = " Engineering-Grade Code Harness";
    let full = format!("{seal}{tagline}");
    let fg = if model.has_modal() {
        theme::STEEL_RAMP[4]
    } else {
        theme::BRAND
    };
    let style = Style::default().fg(fg);
    // Measure in display cells and keep one cell of slack (strict `<`) so the seal never overflows on
    // terminals that render the brand glyph wider than its nominal width.
    let line = if display_width(&full) < area.width as usize {
        Line::from(vec![
            Span::styled(seal, style),
            Span::styled(tagline, style),
        ])
    } else {
        Line::styled(seal, style)
    };
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
