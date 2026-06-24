use ratatui::Frame;
use ratatui::widgets::Block;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::view_state::APPROVAL_OPTIONS;
use crate::modules::tui::infrastructure::layout::frame_layout;
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::{
    approval, command_menu, editor, header, hint_line, meta_rule, transcript_pane,
};

/// The sole ratatui render entry point: project the model onto the frame's regions. Pure with respect
/// to the model (takes `&Model`); all state changes happen in `update`. The whole frame is first painted
/// with the Tamahagane Void base so every cell inherits steel-on-void.
pub fn view(model: &Model, frame: &mut Frame) {
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let wrap_w = frame
        .area()
        .width
        .saturating_sub(editor::PROMPT_COLS)
        .max(1) as usize;
    let input_lines = editor::wrapped_line_count(&model.input, wrap_w) as u16;
    // A pending plan/approval box owns a dedicated region directly above the input, so it is always
    // anchored to the bottom — never carved out of the scrolling transcript. Size it here so the layout
    // can reserve exactly the rows it needs.
    let box_h = if model.pending_plan.is_some() {
        approval::box_dims(
            frame.area(),
            approval::PLAN_ACTION,
            approval::plan_options_len(),
        )
        .1
    } else if let Some(pending) = &model.pending_approval {
        approval::box_dims(frame.area(), pending.action(), APPROVAL_OPTIONS.len()).1
    } else {
        0
    };
    let regions = frame_layout(frame.area(), input_lines, box_h);
    header::render(model, frame, regions.header);
    transcript_pane::render(model, frame, regions.transcript);
    if let Some(plan) = &model.pending_plan {
        approval::render_plan_into(plan, frame, regions.prompt_box);
    } else if let Some(pending) = &model.pending_approval {
        approval::render(pending, frame, regions.prompt_box);
    }
    meta_rule::render(model, frame, regions.meta);
    editor::render(model, frame, regions.input);
    hint_line::render(model, frame, regions.hint);
    // The slash-command preview floats just above the editor while the buffer is a `/`-prefixed token.
    if let Some(menu) = &model.command_menu {
        command_menu::render(menu, frame, regions.input);
    }
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
        model.pending_approval = Some(PendingApproval::new("ler a.txt".to_string(), true));
        let out = render(&model, 80, 20);
        assert!(out.contains("você › oi"), "user item missing:\n{out}");
        assert!(out.contains("olá"), "assistant item missing:\n{out}");
        assert!(
            out.contains("aprovação"),
            "approval box title missing:\n{out}"
        );
        assert!(out.contains("ler a.txt"), "approval action missing:\n{out}");
        assert!(out.contains("Sim"), "approval option missing:\n{out}");
    }

    #[test]
    fn meta_rule_shows_the_active_approval_mode() {
        use crate::modules::agent::application::approval_policy::ApprovalMode;
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.approval_mode = ApprovalMode::Plan;
        let out = render(&model, 80, 12);
        assert!(out.contains("PLAN"), "mode badge missing:\n{out}");
    }

    #[test]
    fn pending_plan_renders_the_plan_box_below_the_transcript() {
        use crate::modules::tui::domain::view_state::PendingPlan;
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model
            .transcript
            .push(TranscriptItem::Assistant("meu plano".to_string()));
        model.pending_plan = Some(PendingPlan::default());
        let out = render(&model, 80, 20);
        assert!(out.contains("plano"), "plan box title missing:\n{out}");
        assert!(out.contains("Executar"), "plan option missing:\n{out}");
        // The box must sit BELOW the transcript, not overlay it: the assistant's plan text renders on
        // an earlier row than the box's options.
        let rows: Vec<&str> = out.lines().collect();
        let plan_row = rows.iter().position(|l| l.contains("meu plano"));
        let box_row = rows.iter().position(|l| l.contains("Executar"));
        assert!(
            matches!((plan_row, box_row), (Some(p), Some(b)) if p < b),
            "plan text should render above the box (plan_row={plan_row:?}, box_row={box_row:?}):\n{out}"
        );
        // ...and the box sits in its dedicated region just above the input prompt, anchored to the
        // bottom — the box options render below the transcript but above the `›_` prompt row.
        let prompt_row = rows.iter().position(|l| l.contains("›_"));
        assert!(
            matches!((box_row, prompt_row), (Some(b), Some(p)) if b < p),
            "plan box should sit above the input (box_row={box_row:?}, prompt_row={prompt_row:?}):\n{out}"
        );
    }

    #[test]
    fn plan_box_shows_all_options_on_a_short_terminal() {
        use crate::modules::tui::domain::view_state::PendingPlan;
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.pending_plan = Some(PendingPlan::default());
        // On a short terminal the box takes priority over the transcript, so every option the user
        // must read to answer stays visible (regression: it used to be clipped). Four options plus the
        // surrounding chrome need a row more than three did.
        let out = render(&model, 80, 12);
        assert!(out.contains("Executar o plano"), "option 1 missing:\n{out}");
        assert!(out.contains("modo auto"), "option 2 missing:\n{out}");
        assert!(out.contains("Continuar"), "option 3 missing:\n{out}");
        assert!(out.contains("Cancelar"), "option 4 clipped:\n{out}");
    }

    #[test]
    fn typed_input_renders_in_the_editor() {
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.input.set("ola mundo".to_string());
        let out = render(&model, 80, 12);
        assert!(out.contains("ola mundo"), "input text missing:\n{out}");
    }

    #[test]
    fn narrow_terminal_keeps_prompt_and_short_hint_without_overflow() {
        let model = Model::new("m".to_string(), "/w".to_string());
        let out = render(&model, 20, 8);
        // The prompt glyph and the seal survive; the long hint collapses to the short form.
        assert!(out.contains("kiri"), "brand seal missing:\n{out}");
        assert!(out.contains("›_"), "prompt glyph missing:\n{out}");
        assert!(out.contains("/help"), "short hint missing:\n{out}");
    }
}
