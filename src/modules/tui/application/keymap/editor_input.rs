use super::*;

/// Double-tap window for Ctrl+C (quit) and Esc (cancel): two presses within this interval count as
/// a double. Tuned to feel deliberate without being sluggish.
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(500);
/// Multi-click window: presses on the same cell within this interval escalate the selection granularity
/// (char → word → line), the way a notepad's double/triple click does.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// Interpret a key press against the current model, mutating it and returning any effects. Pure: no
/// I/O, so it is fully unit-testable. While an approval is pending, keys answer it; otherwise they
/// drive the editor, history, and scrollback.
pub fn on_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if model.pending_approval.is_some() {
        return on_approval_key(model, key);
    }
    if model.pending_plan.is_some() {
        return on_plan_key(model, key);
    }
    if model.picker.is_some() {
        return on_picker_key(model, key);
    }
    if model.wizard.is_some() {
        return on_wizard_key(model, key);
    }

    // A live screen selection takes Ctrl+C first: request a copy (the runtime scrapes the buffer) and
    // clear afterward, so a second Ctrl+C resumes cancel/quit. This precedes both the composer's own
    // Ctrl+C copy and the generic clear-on-key below; `last_ctrl_c` is left untouched so it cannot
    // interfere with the quit double-tap.
    if key.ctrl
        && !key.alt
        && key.code == Key::Char('c')
        && model.selection.active.is_some_and(|s| !s.is_empty())
    {
        if let Some(sel) = model.selection.active.as_mut() {
            sel.state = SelectionState::CopyAndClear;
        }
        return vec![];
    }
    // Any other key means the user is editing or navigating — drop the screen highlight now (the copy
    // path above already returned).
    model.selection.active = None;

    // Before anything else: the menu intercepts navigation and completion keys, but lets ordinary
    // typing fall through to the editor (which keeps the filter in sync after each mutation).
    if model.command_menu.is_some()
        && let Some(effects) = on_menu_key(model, &key)
    {
        return effects;
    }

    // Windows-style chords for clipboard/undo and session control take precedence over text input, and
    // override the widget's emacs defaults (Ctrl+A=head, Ctrl+V=paste, Ctrl+Y=paste, ...). Clipboard is
    // I/O, so the reducer only mutates the buffer and emits an effect; the runtime performs the I/O.
    if key.ctrl && !key.alt {
        match key.code {
            Key::Char('c') => {
                if let Some(text) = model.input.copy_selection() {
                    return vec![Effect::CopyToClipboard(text)];
                }
                let now = Instant::now();
                let double_tap = model
                    .timeline
                    .last_ctrl_c
                    .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_WINDOW);
                model.timeline.last_ctrl_c = Some(now);
                if double_tap {
                    model.should_quit = true;
                    return vec![Effect::Quit];
                }
                if model.busy {
                    return vec![Effect::CancelTurn];
                }
                return vec![];
            }
            Key::Char('x') => {
                return match model.input.cut_selection() {
                    Some(text) => {
                        sync_menu(model);
                        vec![Effect::CopyToClipboard(text)]
                    }
                    None => vec![],
                };
            }
            Key::Char('v') => return vec![Effect::PasteClipboard],
            // Ctrl+A is intentionally NOT select-all: on macOS it is "move to line start" (Cocoa /
            // readline standard), so it falls through to the editor (the widget binds Ctrl+A -> head).
            // Select-all has no single key (Cmd+A never reaches a TTY) — use the mouse or Shift+motions.
            Key::Char('z') => {
                model.input.undo();
                sync_menu(model);
                return vec![];
            }
            Key::Char('y') => {
                model.input.redo();
                sync_menu(model);
                return vec![];
            }
            // Toggle full vs preview-bounded tool output / edit diffs. A pure view flag, live even
            // mid-turn so the user can expand a long output while it streams.
            Key::Char('o') => {
                model.expand_tools = !model.expand_tools;
                return vec![];
            }
            Key::Char('d') if model.input.is_empty() => {
                model.should_quit = true;
                return vec![Effect::Quit];
            }
            _ => {}
        }
    }

    match key.code {
        // Double Esc while busy cancels the current turn (alternative to Ctrl+C).
        // Single Esc while busy is a no-op (recorded for the double-tap window).
        Key::Esc if model.busy => {
            let now = Instant::now();
            let double_tap = model
                .timeline
                .last_esc
                .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_WINDOW);
            model.timeline.last_esc = Some(now);
            if double_tap {
                return vec![Effect::CancelTurn];
            }
            vec![]
        }
        // Shift/Alt+Enter inserts a newline; plain Enter submits.
        Key::Enter if key.shift || key.alt => {
            model.input.newline();
            sync_menu(model);
            vec![]
        }
        Key::Enter => submit(model),
        // Shift+Tab cycles the approval mode (Default -> Auto -> Plan); ignored mid-turn, since the
        // mode is read when a turn starts.
        Key::BackTab => {
            if !model.busy {
                model.approval_mode = model.approval_mode.next();
            }
            vec![]
        }
        // Ctrl+Home/End jump the transcript scrollback; plain Home/End fall through to the editor.
        Key::Home if key.ctrl => {
            model.scroll.top();
            vec![]
        }
        Key::End if key.ctrl => {
            model.scroll.pin();
            vec![]
        }
        // Up/Down recall history only at the buffer's edges; inside a multi-line buffer they move the
        // cursor between lines (handled by the editor fall-through). Shift or an active selection always
        // means "select", never "recall".
        Key::Up if !key.shift && !model.input.is_selecting() && model.input.cursor_row() == 0 => {
            if let Some(line) = model.history.older(&model.input.text()) {
                model.input.set(line);
                sync_menu(model);
            }
            vec![]
        }
        Key::Down
            if !key.shift
                && !model.input.is_selecting()
                && model.input.cursor_row() == model.input.last_row() =>
        {
            if let Some(line) = model.history.newer() {
                model.input.set(line);
                sync_menu(model);
            }
            vec![]
        }
        // PageUp/PageDown: coarse transcript scroll; shifted scrolls a page instead of a step.
        Key::PageUp => {
            model
                .scroll
                .up(if key.shift { SCROLL_PAGE } else { SCROLL_STEP });
            vec![]
        }
        Key::PageDown => {
            model
                .scroll
                .down(if key.shift { SCROLL_PAGE } else { SCROLL_STEP });
            vec![]
        }
        // Everything else — typing, deletion, cursor motion, word motion, Shift-selection — is the
        // editor's; the widget handles it and we keep the slash-command preview in sync.
        _ => {
            model.input.feed_key(key);
            sync_menu(model);
            vec![]
        }
    }
}

/// Interpret a left mouse gesture, mutating the screen selection. Pure: the clock comes from
/// `model.timeline.last_event_at` (stamped by the runtime when the event arrived), and word/line ranges are
/// derived later from the rendered buffer — here we only set anchor/head and the granularity intent.
/// Not gated by a pending modal: selecting and copying the text of an approval/plan box is allowed.
pub fn on_mouse(model: &mut Model, kind: MouseKind, col: u16, row: u16) -> Vec<Effect> {
    match kind {
        MouseKind::Down => {
            let granularity = click_granularity(model, col, row);
            model.selection.active = Some(ScreenSelection::new(col, row, granularity));
        }
        MouseKind::Drag => {
            // A drag is always a character-range gesture, even if it began as a double-click.
            if let Some(sel) = model.selection.active.as_mut() {
                sel.granularity = Granularity::Char;
                sel.extend(col, row);
            }
        }
        MouseKind::Up => {
            let bare = match model.selection.active.as_mut() {
                Some(sel) => {
                    if sel.granularity == Granularity::Char {
                        sel.extend(col, row);
                    }
                    sel.is_empty()
                }
                None => false,
            };
            if bare {
                // A bare click (down+up on one cell) selects nothing — leave no stray highlight, and in
                // the focused composer ask the runtime to drop the edit cursor where it landed (the
                // runtime owns the render geometry; under a modal the editor is read-only, so do nothing).
                model.selection.active = None;
                if !model.has_modal() {
                    return vec![Effect::PlaceCursor { col, row }];
                }
            } else if let Some(sel) = model.selection.active.as_mut() {
                sel.state = SelectionState::CopyAndKeep;
            }
        }
    }
    vec![]
}

/// Classify a mouse-down as a single/double/triple click, escalating the granularity when the same cell
/// is pressed again within `MULTI_CLICK_WINDOW`. The running count lives in `last_click` (not in the
/// selection, which a bare click clears between presses).
fn click_granularity(model: &mut Model, col: u16, row: u16) -> Granularity {
    let now = model.timeline.last_event_at;
    let count = match (now, model.selection.last_click) {
        (Some(now), Some((prev, pos, n)))
            if pos == (col, row) && now.duration_since(prev) < MULTI_CLICK_WINDOW =>
        {
            n.saturating_add(1)
        }
        _ => 1,
    };
    model.selection.last_click = now.map(|t| (t, (col, row), count));
    match count {
        1 => Granularity::Char,
        2 => Granularity::Word,
        _ => Granularity::Line,
    }
}
