use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::modules::tui::application::msg::{Key, KeyPress, Msg};

/// Translate a crossterm event into a `Msg`, or `None` for events the TUI ignores. Only key *press*
/// events are forwarded (key-release events, reported by some terminals, would double every keystroke).
/// Mouse wheel scroll up/down maps to transcript scroll messages.
pub fn to_msg(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => key_to_msg(key),
        Event::Paste(text) => Some(Msg::Paste(text)),
        Event::Resize(..) => Some(Msg::Resize),
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp => Some(Msg::ScrollUp),
            MouseEventKind::ScrollDown => Some(Msg::ScrollDown),
            _ => None,
        },
        _ => None,
    }
}

fn key_to_msg(key: KeyEvent) -> Option<Msg> {
    let code = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        _ => return None,
    };
    let modifiers = key.modifiers;
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let alt = modifiers.contains(KeyModifiers::ALT);
    // Windows surfaces AltGr as Ctrl+Alt. A character carrying that combo is text the OS already
    // composed for the active keyboard layout (ç, á, €, …), not a chord — drop both modifiers so it
    // reaches the editor as ordinary input instead of being mistaken for an unbound shortcut and lost.
    let is_altgr_text = matches!(code, Key::Char(_)) && ctrl && alt;
    Some(Msg::Key(KeyPress {
        code,
        ctrl: ctrl && !is_altgr_text,
        alt: alt && !is_altgr_text,
        shift: modifiers.contains(KeyModifiers::SHIFT),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::application::msg::Key;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn keypress(code: KeyCode, modifiers: KeyModifiers) -> KeyPress {
        match to_msg(Event::Key(KeyEvent::new(code, modifiers))) {
            Some(Msg::Key(press)) => press,
            other => panic!("expected a key press, got {other:?}"),
        }
    }

    #[test]
    fn altgr_char_is_delivered_as_plain_text() {
        // The AltGr-composed cedilla must reach the editor, not be dropped as a Ctrl+Alt chord.
        let press = keypress(
            KeyCode::Char('ç'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(press.code, Key::Char('ç'));
        assert!(!press.ctrl && !press.alt);
    }

    #[test]
    fn ctrl_only_char_keeps_its_modifier() {
        // A real Ctrl chord (no Alt) is left untouched for the key map to act on.
        let press = keypress(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(press.code, Key::Char('a'));
        assert!(press.ctrl && !press.alt);
    }
}
