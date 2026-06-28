use super::*;

/// Open, refresh, or close the slash-command preview based on the current buffer. The menu is gated:
/// never open during a running turn, while an approval/plan box is up, or once the input contains
/// whitespace (the user moved on to arguments). Allowed only while the buffer starts with `/`.
pub fn sync_menu(model: &mut Model) {
    let text = model.input.text();
    let can_open = !model.busy
        && model.pending_approval.is_none()
        && model.pending_plan.is_none()
        && model.picker.is_none()
        && model.wizard.is_none()
        && text.starts_with('/')
        && !text.chars().any(char::is_whitespace);
    if !can_open {
        model.command_menu = None;
        return;
    }
    match &mut model.command_menu {
        Some(menu) => menu.refresh(&text),
        slot @ None => *slot = Some(CommandMenu::open(&text)),
    }
}

/// Handle keys that the menu owns while it is open. Returns `Some(effects)` when the key is consumed
/// (Up/Down/Tab/Esc), or `None` to fall through to the editor — typing keys still update the filter
/// via `sync_menu` after the editor mutation.
pub(super) fn on_menu_key(model: &mut Model, key: &KeyPress) -> Option<Vec<Effect>> {
    if key.ctrl {
        return None;
    }
    match key.code {
        Key::Up => {
            if let Some(menu) = model.command_menu.as_mut() {
                menu.move_cursor(-1);
            }
            Some(vec![])
        }
        Key::Down => {
            if let Some(menu) = model.command_menu.as_mut() {
                menu.move_cursor(1);
            }
            Some(vec![])
        }
        Key::Tab => {
            if let Some(menu) = model.command_menu.as_ref()
                && let Some(spec) = menu.spec()
            {
                complete_command(model, spec.name);
            }
            Some(vec![])
        }
        Key::Esc => {
            model.command_menu = None;
            Some(vec![])
        }
        _ => None,
    }
}

/// Replace the slash-command token in the buffer with `name` followed by a single space (Tab moves to
/// argument mode), then close the menu. Uses `set` to keep `InputBuffer`'s cursor on a char boundary.
fn complete_command(model: &mut Model, name: &'static str) {
    let mut new_text = String::with_capacity(name.len() + 1);
    new_text.push_str(name);
    new_text.push(' ');
    model.input.set(new_text);
    model.command_menu = None;
}
