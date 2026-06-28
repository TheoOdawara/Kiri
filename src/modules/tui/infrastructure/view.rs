use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Block;

use crate::modules::tui::domain::modal::ApprovalOption;
use crate::modules::tui::domain::model::{ActiveModal, Model};
use crate::modules::tui::infrastructure::layout::{Regions, frame_layout, h_pad};
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::{
    approval, command_menu, editor, header, hint_line, meta_rule, selection_overlay,
    transcript_pane, wizard,
};

/// The sole ratatui render entry point: project the model onto the frame's regions. Pure with respect
/// to the model (takes `&Model`); all state changes happen in `update`. The whole frame is first painted
/// with the Tamahagane Void base so every cell inherits steel-on-void.
pub fn view(model: &Model, frame: &mut Frame) {
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let area = frame.area();
    // Fold per-frame geometry into the session motion preference: a short or narrow terminal freezes
    // motion (the layout stays identical, just steady).
    let motion = model
        .motion
        .and_reduce_if(area.height < 8 || area.width < 60);
    let regions = frame_regions(area, model);
    header::render(model, frame, regions.header);
    transcript_pane::render(model, frame, regions.transcript, motion);
    match model.active_modal() {
        Some(ActiveModal::Plan(plan)) => {
            approval::render_plan_into(plan, frame, regions.prompt_box)
        }
        Some(ActiveModal::Approval(pending)) => {
            approval::render(pending, frame, regions.prompt_box)
        }
        Some(ActiveModal::Picker(picker)) => {
            approval::render_picker(picker, frame, regions.prompt_box)
        }
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
    // The screen text selection paints last, over everything, so a drag can highlight any region of the
    // rendered UI. It restyles cells without touching their symbols, so the runtime can scrape the copy.
    if let Some(sel) = model.selection {
        let area = frame.area();
        selection_overlay::paint(frame.buffer_mut(), area, &sel, theme::selection());
    }
}

/// Resolve the frame's stacked regions for the current model: the input height grows with the wrapped
/// buffer, and a pending plan/approval box reserves its slot directly above the input. Shared by `view`
/// (to render) and the runtime (to map a composer click back to a cursor position), so both agree on the
/// geometry byte-for-byte.
pub fn frame_regions(area: Rect, model: &Model) -> Regions {
    // Match the editor's real content width: the side gutters reduce it by `2 * h_pad`, and the prompt
    // gutter takes `PROMPT_COLS` more, so the wrapped-line count stays in step with what is rendered.
    let wrap_w = area
        .width
        .saturating_sub(2 * h_pad(area))
        .saturating_sub(editor::PROMPT_COLS)
        .max(1) as usize;
    let input_lines = editor::wrapped_line_count(&model.input, wrap_w) as u16;
    // A pending plan/approval stanza owns a dedicated region directly above the input, so it is always
    // anchored to the bottom — never carved out of the scrolling transcript. Size it against the same
    // gutter-inset content width the stanza actually renders at, so a long action never under-reserves.
    let content = Rect {
        width: area.width.saturating_sub(2 * h_pad(area)),
        ..area
    };
    let box_h = match model.active_modal() {
        Some(ActiveModal::Plan(_)) => {
            approval::box_dims(content, approval::PLAN_ACTION, approval::plan_options_len()).1
        }
        Some(ActiveModal::Approval(pending)) => {
            approval::box_dims(content, pending.action(), ApprovalOption::ALL.len()).1
        }
        Some(ActiveModal::Picker(picker)) => {
            approval::box_dims(content, &picker.action, picker.options.len()).1
        }
        Some(ActiveModal::Wizard(provider_wizard)) => wizard::box_dims(content, provider_wizard).1,
        None => 0,
    };
    frame_layout(area, input_lines, box_h)
}

#[path = "view_tests.rs"]
#[cfg(test)]
mod tests;
