use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Block;

use crate::modules::tui::domain::modal::ApprovalOption;
use crate::modules::tui::domain::model::{ActiveModal, Model};
use crate::modules::tui::infrastructure::layout::{Regions, frame_layout, h_pad_total};
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::{
    approval, command_menu, editor, header, hint_line, meta_rule, picker, plan_pane, search_bar,
    selection_overlay, sidebar, transcript_pane, wizard,
};

/// The sole ratatui render entry point: project the model onto the frame's regions. Pure with respect
/// to the model (takes `&Model`); all state changes happen in `update`. The whole frame is first painted
/// with the Tamahagane Void base so every cell inherits steel-on-void.
pub fn view(model: &Model, frame: &mut Frame) {
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let mut area = frame.area();

    // Add a top margin of 1 line on roomy terminals so everything is not glued to the top edge.
    if area.height >= 12 {
        area.y += 1;
        area.height = area.height.saturating_sub(1);
    }

    let has_sidebar = area.width >= 130;
    let (main_area, sidebar_area) = if has_sidebar {
        let split = ratatui::layout::Layout::horizontal([
            ratatui::layout::Constraint::Min(90),
            ratatui::layout::Constraint::Length(35),
        ])
        .split(area);
        (split[0], Some(split[1]))
    } else {
        (area, None)
    };

    // Fold per-frame geometry into the session motion preference: a short or narrow terminal freezes
    // motion (the layout stays identical, just steady).
    let motion = model
        .timeline
        .motion
        .and_reduce_if(main_area.height < 8 || main_area.width < 60);
    let regions = frame_regions(main_area, model);
    header::render(model, frame, regions.header);

    if model.pending_plan.is_some() {
        let transcript_area = regions.transcript;
        if transcript_area.width >= 100 {
            // Split horizontally: 40% transcript, 60% plan
            let split = ratatui::layout::Layout::horizontal([
                ratatui::layout::Constraint::Percentage(40),
                ratatui::layout::Constraint::Min(2), // 2 columns spacer
                ratatui::layout::Constraint::Percentage(60),
            ])
            .split(transcript_area);

            transcript_pane::render(model, frame, split[0], motion);
            plan_pane::render(model, frame, split[2]);
        } else {
            // Replace transcript completely
            plan_pane::render(model, frame, transcript_area);
        }
    } else {
        transcript_pane::render(model, frame, regions.transcript, motion);
    }

    match model.active_modal() {
        Some(ActiveModal::Plan(plan)) => {
            approval::render_plan_into(plan, frame, regions.prompt_box)
        }
        Some(ActiveModal::Approval(pending)) => {
            approval::render(pending, frame, regions.prompt_box)
        }
        Some(ActiveModal::Picker(picker)) => picker::render(picker, frame, main_area),
        Some(ActiveModal::Wizard(provider_wizard)) => {
            wizard::render(provider_wizard, frame, regions.prompt_box)
        }
        None => {}
    }
    meta_rule::render(model, frame, regions.meta);
    editor::render(model, frame, regions.input, motion);
    hint_line::render(model, frame, regions.hint);
    // The slash-command preview floats just above the editor while the buffer is a `/`-prefixed token.
    if let Some(menu) = &model.command_menu {
        command_menu::render(menu, frame, regions.input);
    }
    if model.search_query.is_some() {
        search_bar::render(model, frame, regions.input);
    }
    if let Some(sb_area) = sidebar_area {
        sidebar::render(model, frame, sb_area);
    }
    // The screen text selection paints last, over everything, so a drag can highlight any region of the
    // rendered UI. It restyles cells without touching their symbols, so the runtime can scrape the copy.
    if let Some(sel) = model.selection.active {
        let area = frame.area();
        selection_overlay::paint(frame.buffer_mut(), area, &sel, theme::selection());
    }
}

/// Resolve the frame's stacked regions for the current model: the input height grows with the wrapped
/// buffer, and a pending plan/approval box reserves its slot directly above the input. Shared by `view`
/// (to render) and the runtime (to map a composer click back to a cursor position), so both agree on the
/// geometry byte-for-byte.
pub fn frame_regions(area: Rect, model: &Model) -> Regions {
    // Match the editor's real content width: the side gutters reduce it by `h_pad_total`, and the prompt
    // gutter takes `PROMPT_COLS` more, so the wrapped-line count stays in step with what is rendered.
    let total_h_pad = h_pad_total(area);
    let wrap_w = area
        .width
        .saturating_sub(total_h_pad)
        .saturating_sub(editor::PROMPT_COLS)
        .max(1) as usize;
    let input_lines = editor::wrapped_line_count(&model.input, wrap_w) as u16;
    // A pending plan/approval stanza owns a dedicated region directly above the input, so it is always
    // anchored to the bottom — never carved out of the scrolling transcript. Size it against the same
    // gutter-inset content width the stanza actually renders at, so a long action never under-reserves.
    let content = Rect {
        width: area.width.saturating_sub(total_h_pad),
        ..area
    };
    let box_h = match model.active_modal() {
        Some(ActiveModal::Plan(_)) => {
            approval::box_dims(content, approval::PLAN_ACTION, approval::plan_options_len()).1
        }
        Some(ActiveModal::Approval(pending)) => {
            approval::box_dims(content, pending.action(), ApprovalOption::ALL.len()).1
        }
        Some(ActiveModal::Picker(_)) => 0,
        Some(ActiveModal::Wizard(provider_wizard)) => wizard::box_dims(content, provider_wizard).1,
        None => 0,
    };
    frame_layout(area, input_lines, box_h)
}

#[path = "view_tests.rs"]
#[cfg(test)]
mod tests;
