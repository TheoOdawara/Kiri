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

/// Copy text to the OS clipboard, surfacing a failure as a transcript notice — copy is a direct user
/// intent, so it must never fail silently. An empty text is a no-op (the clipboard is left untouched).
pub(super) fn copy_to_clipboard(model: &mut Model, text: &str) {
    if let Err(error) = clipboard::copy_text(text) {
        model.notify_error(format!(
            "falha ao copiar para a área de transferência: {error}"
        ));
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
