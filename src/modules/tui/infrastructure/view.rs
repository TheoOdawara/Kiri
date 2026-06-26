use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Block;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::view_state::APPROVAL_OPTIONS;
use crate::modules::tui::infrastructure::layout::{frame_layout, h_pad};
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::{
    approval, command_menu, editor, header, hint_line, meta_rule, transcript_pane,
};

/// The sole ratatui render entry point: project the model onto the frame's regions. Pure with respect
/// to the model (takes `&Model`); all state changes happen in `update`. The whole frame is first painted
/// with the Tamahagane Void base so every cell inherits steel-on-void.
pub fn view(model: &Model, frame: &mut Frame) {
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    // Match the editor's real content width: the side gutters reduce it by `2 * h_pad`, and the prompt
    // gutter takes `PROMPT_COLS` more, so the wrapped-line count stays in step with what is rendered.
    let wrap_w = frame
        .area()
        .width
        .saturating_sub(2 * h_pad(frame.area()))
        .saturating_sub(editor::PROMPT_COLS)
        .max(1) as usize;
    let input_lines = editor::wrapped_line_count(&model.input, wrap_w) as u16;
    // A pending plan/approval stanza owns a dedicated region directly above the input, so it is always
    // anchored to the bottom — never carved out of the scrolling transcript. Size it against the same
    // gutter-inset content width the stanza actually renders at, so a long action never under-reserves.
    let content = Rect {
        width: frame.area().width.saturating_sub(2 * h_pad(frame.area())),
        ..frame.area()
    };
    let box_h = if model.pending_plan.is_some() {
        approval::box_dims(content, approval::PLAN_ACTION, approval::plan_options_len()).1
    } else if let Some(pending) = &model.pending_approval {
        approval::box_dims(content, pending.action(), APPROVAL_OPTIONS.len()).1
    } else {
        0
    };
    // Fold per-frame geometry into the session motion preference: a short or narrow terminal freezes
    // motion (the layout stays identical, just steady).
    let area = frame.area();
    let motion = model
        .motion
        .and_reduce_if(area.height < 8 || area.width < 60);
    let regions = frame_layout(frame.area(), input_lines, box_h);
    header::render(model, frame, regions.header);
    transcript_pane::render(model, frame, regions.transcript, motion);
    if let Some(plan) = &model.pending_plan {
        approval::render_plan_into(plan, frame, regions.prompt_box);
    } else if let Some(pending) = &model.pending_approval {
        approval::render(pending, frame, regions.prompt_box);
    }
    meta_rule::render(model, frame, regions.meta);
    editor::render(model, frame, regions.input, motion);
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
        assert!(out.contains("›▏"), "prompt glyph missing:\n{out}");
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
    fn streaming_answer_shows_the_wet_ink_caret() {
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model
            .transcript
            .push(TranscriptItem::Assistant("escrevendo".to_string()));
        model.status.streaming = true;
        let out = render(&model, 80, 20);
        assert!(out.contains('▌'), "wet-ink caret missing:\n{out}");
    }

    #[test]
    fn active_streaming_item_is_plain_then_formats_when_finished() {
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model
            .transcript
            .push(TranscriptItem::Assistant("**negrito**".to_string()));

        // While streaming, the trailing item renders as plain text — the markdown markers stay literal
        // (no per-frame parse).
        model.status.streaming = true;
        let streaming = render(&model, 100, 30);
        assert!(
            streaming.contains("**negrito**"),
            "streaming item must render plain (literal markers):\n{streaming}"
        );

        // Once the turn finishes, the same item re-renders formatted: the `**` markers are gone.
        model.status.streaming = false;
        let finished = render(&model, 100, 30);
        assert!(
            finished.contains("negrito") && !finished.contains("**negrito**"),
            "finished item must render markdown (markers stripped):\n{finished}"
        );
    }

    #[test]
    fn meta_rule_shows_the_active_approval_mode() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
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
        // bottom — the box options render below the transcript but above the `›▏` prompt row.
        let prompt_row = rows.iter().position(|l| l.contains("›▏"));
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
    fn tool_activity_renders_command_result_and_diff() {
        use crate::modules::tui::domain::transcript::{ToolActivity, ToolDiff, ToolStatus};
        use std::time::Duration;
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.transcript.push(TranscriptItem::Tool(ToolActivity {
            command: "edit src/app.rs".to_string(),
            diff: Some(ToolDiff {
                old: "let mode = mode;".to_string(),
                new: "let mut mode = mode;".to_string(),
            }),
            result: Some((
                ToolStatus::Ok,
                "edited src/app.rs".to_string(),
                Duration::from_millis(7),
            )),
        }));
        let out = render(&model, 80, 20);
        assert!(
            out.contains("⏺ edit src/app.rs"),
            "command line missing:\n{out}"
        );
        assert!(out.contains("⎿"), "result marker missing:\n{out}");
        assert!(
            out.contains("- let mode = mode;"),
            "removed diff line missing:\n{out}"
        );
        assert!(
            out.contains("+ let mut mode = mode;"),
            "added diff line missing:\n{out}"
        );
        assert!(
            out.contains("edited src/app.rs"),
            "result detail missing:\n{out}"
        );
    }

    #[test]
    fn long_tool_output_is_previewed_until_expanded() {
        use crate::modules::tui::domain::transcript::{ToolActivity, ToolStatus};
        use std::time::Duration;
        let output = (1..=20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.transcript.push(TranscriptItem::Tool(ToolActivity {
            command: "cat big.txt".to_string(),
            diff: None,
            result: Some((ToolStatus::Ok, output, Duration::from_millis(1))),
        }));
        let collapsed = render(&model, 80, 40);
        assert!(
            collapsed.contains("para expandir"),
            "elision hint missing when collapsed:\n{collapsed}"
        );
        model.expand_tools = true;
        let expanded = render(&model, 80, 40);
        assert!(
            expanded.contains("line20"),
            "expanded output should show every line:\n{expanded}"
        );
        assert!(
            !expanded.contains("para expandir"),
            "no elision hint once expanded:\n{expanded}"
        );
    }

    #[test]
    fn meta_rule_keeps_mode_badge_with_a_long_workspace() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut model = Model::new(
            "some-long-model-name".to_string(),
            "C:/Users/dev/very/deep/workspace/path/kiri".to_string(),
        );
        model.approval_mode = ApprovalMode::Auto;
        // On a tight width the overlong workspace must not push the mode badge off the rule.
        let out = render(&model, 40, 12);
        assert!(
            out.contains("AUTO"),
            "mode badge must survive a long workspace:\n{out}"
        );
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
        assert!(out.contains("›▏"), "prompt glyph missing:\n{out}");
        assert!(out.contains("/help"), "short hint missing:\n{out}");
    }
}
