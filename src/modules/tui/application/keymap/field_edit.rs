//! Shared clipboard/undo chords for every free-text field (`InputBuffer`) — composer, wizard draft,
//! picker search, and Ctrl+F. Keeps Notepad-like shortcuts consistent without triplicating match arms.

use super::*;
use crate::modules::tui::domain::input_buffer::InputBuffer;

/// Handle Ctrl chords that edit a field buffer. Returns `Some(effects)` when the key was consumed
/// (including no-op cut/copy with no selection); `None` when the key should fall through to `feed_key`.
pub(super) fn field_chords(buf: &mut InputBuffer, key: KeyPress) -> Option<Vec<Effect>> {
    if !key.ctrl || key.alt {
        return None;
    }
    match key.code {
        Key::Char('v') => Some(vec![Effect::PasteClipboard]),
        // No selection: `None` so the caller decides (composer double-tap quit, wizard quit, etc.).
        Key::Char('c') => buf
            .copy_selection()
            .map(|text| vec![Effect::CopyToClipboard(text)]),
        Key::Char('x') => Some(match buf.cut_selection() {
            Some(text) => vec![Effect::CopyToClipboard(text)],
            None => vec![],
        }),
        Key::Char('z') => {
            buf.undo();
            Some(vec![])
        }
        Key::Char('y') => {
            buf.redo();
            Some(vec![])
        }
        _ => None,
    }
}
