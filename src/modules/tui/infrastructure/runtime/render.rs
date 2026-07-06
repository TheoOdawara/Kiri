//! Render and clipboard glue for the run loop: draw a frame (scraping a pending selection to the OS
//! clipboard), copy/paste, resolve a composer click to a cursor move, and read the motion preference.

use std::io;

use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::{DefaultTerminal, Terminal};

use super::clipboard::{self, ClipboardContent};
use crate::modules::tui::application::msg::Msg;
use crate::modules::tui::application::update::update;
use crate::modules::tui::domain::model::{Model, Motion};
use crate::modules::tui::domain::selection::SelectionState;
use crate::modules::tui::infrastructure::view::{frame_regions, view};
use crate::modules::tui::infrastructure::widgets::{editor, selection_overlay};
use crate::shared::kernel::error::AgentError;

/// Draw a frame and, if a copy was requested, scrape the just-rendered selection to the OS clipboard.
/// The caller stamps `model.render_at` first (so line landings share the frame instant). Returns the
/// draw error so each caller chooses how to handle it (the main loop propagates; the turn loop must
/// break, never `?`, so its cleanup still runs).
pub(super) fn draw_and_copy(terminal: &mut DefaultTerminal, model: &mut Model) -> io::Result<()> {
    // Lift the pending copy out first (ScreenSelection is `Copy`), so no `&model` borrow is held across
    // the draw and the post-draw mutation below type-checks.
    let pending = model
        .selection
        .active
        .filter(|s| s.state != SelectionState::Idle);
    let completed = terminal.draw(|frame| view(model, frame))?;
    if let Some(sel) = pending {
        // `completed` borrows the terminal, not the model, so scraping it and then mutating the model
        // below is disjoint — no explicit drop needed.
        let text = selection_overlay::scrape(completed.buffer, &sel, completed.area);
        copy_to_clipboard(model, &text);
        // Mouse-release keeps the highlight (just settle the state); Ctrl+C drops it so the next Ctrl+C
        // is free to cancel/quit. Either way the request is consumed exactly once.
        match sel.state {
            SelectionState::CopyAndClear => model.selection.active = None,
            _ => {
                if let Some(s) = model.selection.active.as_mut() {
                    s.state = SelectionState::Idle;
                }
            }
        }
    }
    Ok(())
}

/// Copy text to the OS clipboard, surfacing the outcome as a transcript notice either way — copy is a
/// direct user intent, so it must never be silent in success OR failure (issue #8c: transient copy
/// feedback).
pub(super) fn copy_to_clipboard(model: &mut Model, text: &str) {
    notify_copy_result(model, clipboard::copy_text(text), !text.is_empty());
}

/// The pure "map a copy attempt to a notice" decision, split out of `copy_to_clipboard` so it is testable
/// without touching the real OS clipboard (mirrors `provider_swap::persist_or_notice`'s split of
/// decision from I/O). `copied_something` is `false` for an empty text — `clipboard::copy_text("")` is a
/// deliberate no-op (the clipboard is left untouched), so it gets no "copied" notice either.
fn notify_copy_result(model: &mut Model, result: Result<(), AgentError>, copied_something: bool) {
    match result {
        Ok(()) if copied_something => model.notify_info("copiado para a área de transferência"),
        Ok(()) => {}
        Err(error) => model.notify_error(format!(
            "falha ao copiar para a área de transferência: {error}"
        )),
    }
}

/// Resolve a composer click to a logical cursor move, against the freshly rendered geometry. The runtime
/// owns the only honest source of the editor's rect — it recomputes it from the current terminal size and
/// model, exactly as the last frame did. A click outside the box, or a wrapped/scrolled layout the widget
/// renders ambiguously, resolves to `None` and leaves the cursor put (never mis-placed).
pub(super) fn place_cursor<B: Backend>(
    model: &mut Model,
    terminal: &Terminal<B>,
    col: u16,
    row: u16,
) {
    // Without the terminal size the geometry is unknown, so the click cannot be mapped — a safe no-op
    // (the user can still navigate by key); nothing actionable is dropped silently.
    let Ok(size) = terminal.size() else { return };
    let area = Rect::new(0, 0, size.width, size.height);
    let editor_area = editor::content_rect(frame_regions(area, model).input);
    if let Some((r, c)) = editor::click_to_cursor(&model.input, editor_area, col, row) {
        model.input.set_cursor(r, c);
    }
}

/// Read the OS clipboard and route it into the buffer: an image becomes a staged attachment, text is
/// inserted at the cursor. Best-effort — an empty clipboard is a silent no-op, but a present-but-unencodable
/// image surfaces a Notice (a paste the user intended must not vanish silently).
pub(super) fn paste_from_clipboard(model: &mut Model) {
    // `update` for these messages produces no effects (they only mutate the model), so the returned
    // Vec is intentionally discarded — there is nothing for the runtime to perform.
    match clipboard::read() {
        ClipboardContent::Image(attachment) => {
            let _ = update(model, Msg::ImageAttached(attachment));
        }
        ClipboardContent::Text(text) => {
            let _ = update(model, Msg::Paste(text));
        }
        // An image was on the clipboard but could not be encoded: tell the user instead of doing nothing.
        ClipboardContent::Unreadable => {
            model.notify_error("não consegui ler a imagem da área de transferência");
        }
        ClipboardContent::Empty => {}
    }
}

/// Resolve the session-wide motion preference from the environment: any non-empty `KIRI_REDUCED_MOTION`
/// or `NO_COLOR` freezes motion to a steady, layout-identical UI; otherwise it is fully expressed.
pub(super) fn resolve_motion() -> Motion {
    let set = |key: &str| std::env::var_os(key).is_some_and(|v| !v.is_empty());
    if set("KIRI_REDUCED_MOTION") || set("NO_COLOR") {
        Motion::Reduced
    } else {
        Motion::Full
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
    use crate::shared::kernel::error::AgentError;

    #[test]
    fn a_successful_copy_of_real_text_notifies_success() {
        let mut model = Model::default();
        notify_copy_result(&mut model, Ok(()), true);
        assert_eq!(
            model.transcript.items().last(),
            Some(&TranscriptItem::Notice(
                NoticeLevel::Info,
                "copiado para a área de transferência".to_string()
            )),
            "a real copy must surface transient success feedback (issue #8c)"
        );
    }

    #[test]
    fn a_no_op_copy_of_empty_text_stays_silent() {
        let mut model = Model::default();
        notify_copy_result(&mut model, Ok(()), false);
        assert!(
            model.transcript.is_empty(),
            "nothing was actually copied, so there is nothing to confirm"
        );
    }

    #[test]
    fn a_failed_copy_notifies_the_error() {
        let mut model = Model::default();
        notify_copy_result(
            &mut model,
            Err(AgentError::Io(io::Error::other("boom"))),
            true,
        );
        assert_eq!(
            model.transcript.items().last(),
            Some(&TranscriptItem::Notice(
                NoticeLevel::Error,
                "falha ao copiar para a área de transferência: boom".to_string()
            ))
        );
    }
}
