use ratatui::Frame;
use ratatui::widgets::Block;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::infrastructure::layout::frame_layout;
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::{
    editor, header, hint_line, meta_rule, transcript_pane,
};

/// The sole ratatui render entry point: project the model onto the frame's regions. Pure with respect
/// to the model (takes `&Model`); all state changes happen in `update`. The whole frame is first painted
/// with the Tamahagane Void base so every cell inherits steel-on-void.
pub fn view(model: &Model, frame: &mut Frame) {
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let regions = frame_layout(frame.area(), model.input.line_count() as u16);
    header::render(model, frame, regions.header);
    transcript_pane::render(model, frame, regions.transcript);
    meta_rule::render(model, frame, regions.meta);
    editor::render(model, frame, regions.input);
    hint_line::render(model, frame, regions.hint);
}

#[cfg(test)]
mod tests {
    use super::view;
    use crate::modules::tui::domain::model::Model;
    use crate::modules::tui::domain::transcript::TranscriptItem;
    use crate::modules::tui::domain::view_state::PendingApproval;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Render the model onto an in-memory backend and flatten the buffer to text for `contains` checks.
    fn render(model: &Model, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| view(model, frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn empty_shell_shows_brand_splash_and_hints() {
        let model = Model::new("model-x".to_string(), "/work".to_string());
        let out = render(&model, 80, 12);
        assert!(out.contains("kiri"), "brand seal missing:\n{out}");
        assert!(out.contains("KIRI"), "splash mark missing:\n{out}");
        assert!(out.contains("model-x"), "model missing:\n{out}");
        assert!(out.contains("›_"), "prompt glyph missing:\n{out}");
        assert!(out.contains("Enter envia"), "hints missing:\n{out}");
    }

    #[test]
    fn transcript_and_pending_approval_render() {
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model
            .transcript
            .push(TranscriptItem::User("oi".to_string()));
        model
            .transcript
            .push(TranscriptItem::Assistant("olá".to_string()));
        model.pending_approval = Some(PendingApproval {
            prompt: "ler a.txt".to_string(),
            default_accept: true,
        });
        let out = render(&model, 80, 12);
        assert!(out.contains("você › oi"), "user item missing:\n{out}");
        assert!(out.contains("olá"), "assistant item missing:\n{out}");
        assert!(out.contains("aprovar?"), "approval prompt missing:\n{out}");
        assert!(out.contains("ler a.txt"), "approval text missing:\n{out}");
    }
}
