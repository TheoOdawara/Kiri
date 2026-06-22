use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::modules::tui::application::msg::{Key, KeyPress, Msg};

/// Translate a crossterm event into a `Msg`, or `None` for events the TUI ignores. Only key *press*
/// events are forwarded (key-release events, reported by some terminals, would double every keystroke).
pub fn to_msg(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => key_to_msg(key),
        Event::Paste(text) => Some(Msg::Paste(text)),
        Event::Resize(..) => Some(Msg::Resize),
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
        _ => return None,
    };
    let modifiers = key.modifiers;
    Some(Msg::Key(KeyPress {
        code,
        ctrl: modifiers.contains(KeyModifiers::CONTROL),
        alt: modifiers.contains(KeyModifiers::ALT),
        shift: modifiers.contains(KeyModifiers::SHIFT),
    }))
}
