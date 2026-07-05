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
    if model.search_query.is_some() {
        return on_search_key(model, key);
    }
    if model.focused_pane == PaneFocus::Transcript {
        return on_transcript_key(model, key);
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
            Key::Char('p') => {
                model.focused_pane = match model.focused_pane {
                    PaneFocus::Input => {
                        if model.selected_item.is_none() && !model.transcript.is_empty() {
                            model.selected_item = Some(model.transcript.items().len().saturating_sub(1));
                        }
                        PaneFocus::Transcript
                    }
                    PaneFocus::Transcript => PaneFocus::Input,
                };
                return vec![];
            }
            Key::Char('f') => {
                if model.search_query.is_some() {
                    model.search_query = None;
                    model.search_results.clear();
                } else {
                    model.search_query = Some(String::new());
                    model.search_results.clear();
                    model.active_search_match = 0;
                }
                return vec![];
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
        // Esc when input is empty swaps to transcript focus.
        Key::Esc if model.input.is_empty() => {
            if !model.transcript.is_empty() {
                model.focused_pane = PaneFocus::Transcript;
                if model.selected_item.is_none() {
                    model.selected_item = Some(model.transcript.items().len().saturating_sub(1));
                }
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

fn on_transcript_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if (key.ctrl && key.code == Key::Char('p')) || key.code == Key::Esc {
        model.focused_pane = PaneFocus::Input;
        return vec![];
    }

    let items_len = model.transcript.items().len();
    if items_len == 0 {
        return vec![];
    }

    let mut current = model.selected_item.unwrap_or(items_len - 1);

    match key.code {
        Key::Up | Key::Char('k') => {
            if current > 0 {
                current -= 1;
            } else {
                current = items_len - 1;
            }
            model.selected_item = Some(current);
            model.scroll.up(2);
        }
        Key::Down | Key::Char('j') => {
            if current < items_len - 1 {
                current += 1;
            } else {
                current = 0;
            }
            model.selected_item = Some(current);
            model.scroll.down(2);
        }
        Key::Enter | Key::Char(' ') => {
            if let Some(TranscriptItem::Tool(_)) = model.transcript.items().get(current) {
                if model.expanded_tools_indices.contains(&current) {
                    model.expanded_tools_indices.remove(&current);
                } else {
                    model.expanded_tools_indices.insert(current);
                }
            }
        }
        Key::Char('c') => {
            if let Some(item) = model.transcript.items().get(current) {
                let text = match item {
                    TranscriptItem::User(t) => t.clone(),
                    TranscriptItem::Reasoning(t) => t.clone(),
                    TranscriptItem::Assistant(t) => extract_code_blocks(t),
                    TranscriptItem::Tool(act) => {
                        if let Some((_, out, _)) = &act.result {
                            out.clone()
                        } else {
                            act.command.clone()
                        }
                    }
                    TranscriptItem::Notice(_, t) => t.clone(),
                };
                if !text.is_empty() {
                    return vec![Effect::CopyToClipboard(text)];
                }
            }
        }
        Key::Char('o') => {
            if let Some(TranscriptItem::Tool(act)) = model.transcript.items().get(current) {
                if let Some(path) = extract_filepath(&act.command) {
                    return vec![Effect::OpenFile(path)];
                }
            }
        }
        _ => {}
    }
    vec![]
}

fn extract_filepath(command: &str) -> Option<String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.len() >= 2 {
        let path = parts[1].trim_matches('"').trim_matches('\'');
        Some(path.to_string())
    } else {
        None
    }
}

fn extract_code_blocks(text: &str) -> String {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current_block = Vec::new();
    for line in text.lines() {
        if line.starts_with("```") {
            if in_block {
                blocks.push(current_block.join("\n"));
                current_block.clear();
                in_block = false;
            } else {
                in_block = true;
            }
        } else if in_block {
            current_block.push(line);
        }
    }
    if blocks.is_empty() {
        text.to_string()
    } else {
        blocks.join("\n\n")
    }
}

fn on_search_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    let mut query = model.search_query.clone().unwrap_or_default();
    
    match key.code {
        Key::Esc | Key::Enter => {
            if key.code == Key::Esc {
                model.search_query = None;
                model.search_results.clear();
            } else {
                model.focused_pane = PaneFocus::Input;
            }
            return vec![];
        }
        Key::Up | Key::Char('p') if key.ctrl => {
            if !model.search_results.is_empty() {
                if model.active_search_match > 0 {
                    model.active_search_match -= 1;
                } else {
                    model.active_search_match = model.search_results.len() - 1;
                }
                let idx = model.search_results[model.active_search_match];
                model.selected_item = Some(idx);
                model.scroll.up(2);
            }
        }
        Key::Down | Key::Char('n') if key.ctrl => {
            if !model.search_results.is_empty() {
                if model.active_search_match < model.search_results.len() - 1 {
                    model.active_search_match += 1;
                } else {
                    model.active_search_match = 0;
                }
                let idx = model.search_results[model.active_search_match];
                model.selected_item = Some(idx);
                model.scroll.down(2);
            }
        }
        Key::Backspace => {
            query.pop();
            update_search_results(model, &query);
        }
        Key::Char(c) if !key.ctrl && !key.alt => {
            query.push(c);
            update_search_results(model, &query);
        }
        _ => {}
    }
    vec![]
}

fn update_search_results(model: &mut Model, query: &str) {
    model.search_query = Some(query.to_string());
    model.search_results.clear();
    model.active_search_match = 0;
    
    if query.is_empty() {
        return;
    }
    
    let query_lower = query.to_lowercase();
    for (idx, item) in model.transcript.items().iter().enumerate() {
        let text = match item {
            TranscriptItem::User(t) => t,
            TranscriptItem::Reasoning(t) => t,
            TranscriptItem::Assistant(t) => t,
            TranscriptItem::Tool(act) => &act.command,
            TranscriptItem::Notice(_, t) => t,
        };
        if text.to_lowercase().contains(&query_lower) {
            model.search_results.push(idx);
        }
    }
    
    if !model.search_results.is_empty() {
        model.selected_item = Some(model.search_results[0]);
    }
}


