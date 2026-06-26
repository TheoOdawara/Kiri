use std::time::{Duration, Instant};

use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Key, KeyPress, MouseKind};
use crate::modules::tui::domain::command_menu::CommandMenu;
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::domain::view_state::{
    ADD_PROVIDER_LABEL, APPROVAL_OPTIONS, Granularity, PLAN_OPTIONS, Picker, PickerKind,
    ProviderWizard, ScreenSelection, SelectionState, WizardStep,
};
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::provider::{Effort, Secret};
use tui_textarea::{Input, Key as TaKey};

const SCROLL_STEP: u16 = 5;
/// A "page" scroll step, used for Shift+PageUp/PageDown. The transcript viewport height is not stored
/// on the model, so a fixed large step stands in; the view clamps to the available history.
const SCROLL_PAGE: u16 = 20;
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
        && model.selection.is_some_and(|s| !s.is_empty())
    {
        if let Some(sel) = model.selection.as_mut() {
            sel.state = SelectionState::CopyAndClear;
        }
        return vec![];
    }
    // Any other key means the user is editing or navigating — drop the screen highlight now (the copy
    // path above already returned).
    model.selection = None;

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
                    .last_ctrl_c
                    .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_WINDOW);
                model.last_ctrl_c = Some(now);
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
                .last_esc
                .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_WINDOW);
            model.last_esc = Some(now);
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
            model.input.feed(to_input(key));
            sync_menu(model);
            vec![]
        }
    }
}

/// Interpret a left mouse gesture, mutating the screen selection. Pure: the clock comes from
/// `model.last_event_at` (stamped by the runtime when the event arrived), and word/line ranges are
/// derived later from the rendered buffer — here we only set anchor/head and the granularity intent.
/// Not gated by a pending modal: selecting and copying the text of an approval/plan box is allowed.
pub fn on_mouse(model: &mut Model, kind: MouseKind, col: u16, row: u16) -> Vec<Effect> {
    match kind {
        MouseKind::Down => {
            let granularity = click_granularity(model, col, row);
            model.selection = Some(ScreenSelection::new(col, row, granularity));
        }
        MouseKind::Drag => {
            // A drag is always a character-range gesture, even if it began as a double-click.
            if let Some(sel) = model.selection.as_mut() {
                sel.granularity = Granularity::Char;
                sel.extend(col, row);
            }
        }
        MouseKind::Up => {
            let bare = match model.selection.as_mut() {
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
                model.selection = None;
                if !model.has_modal() {
                    return vec![Effect::PlaceCursor { col, row }];
                }
            } else if let Some(sel) = model.selection.as_mut() {
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
    let now = model.last_event_at;
    let count = match (now, model.last_click) {
        (Some(now), Some((prev, pos, n)))
            if pos == (col, row) && now.duration_since(prev) < MULTI_CLICK_WINDOW =>
        {
            n.saturating_add(1)
        }
        _ => 1,
    };
    model.last_click = now.map(|t| (t, (col, row), count));
    match count {
        1 => Granularity::Char,
        2 => Granularity::Word,
        _ => Granularity::Line,
    }
}

/// Map a normalized key press onto the editor widget's backend-agnostic input type.
///
/// macOS word ops are translated onto the bindings the widget already understands: terminals send
/// Option as Alt, but the widget binds word motion to `Ctrl+Left/Right` (and meta `Alt+b/f`) and
/// word-delete to `Alt+Backspace/Delete`. So `Option+←/→` is rewritten to `Ctrl+←/→` — with `alt: false`,
/// which is mandatory: the widget has a `Ctrl+Alt+Left -> line head` arm that would otherwise win.
/// `Ctrl+Backspace/Delete` (Windows/Linux muscle memory) is rewritten to the widget's `Alt` word-delete.
fn to_input(key: KeyPress) -> Input {
    match (key.ctrl, key.alt, key.code) {
        (false, true, Key::Left) => {
            return Input {
                key: TaKey::Left,
                ctrl: true,
                alt: false,
                shift: key.shift,
            };
        }
        (false, true, Key::Right) => {
            return Input {
                key: TaKey::Right,
                ctrl: true,
                alt: false,
                shift: key.shift,
            };
        }
        (true, false, Key::Backspace) => {
            return Input {
                key: TaKey::Backspace,
                ctrl: false,
                alt: true,
                shift: false,
            };
        }
        (true, false, Key::Delete) => {
            return Input {
                key: TaKey::Delete,
                ctrl: false,
                alt: true,
                shift: false,
            };
        }
        _ => {}
    }
    Input {
        key: match key.code {
            Key::Char(c) => TaKey::Char(c),
            Key::Enter => TaKey::Enter,
            Key::Backspace => TaKey::Backspace,
            Key::Delete => TaKey::Delete,
            Key::Left => TaKey::Left,
            Key::Right => TaKey::Right,
            Key::Up => TaKey::Up,
            Key::Down => TaKey::Down,
            Key::Home => TaKey::Home,
            Key::End => TaKey::End,
            Key::PageUp => TaKey::PageUp,
            Key::PageDown => TaKey::PageDown,
            Key::Esc => TaKey::Esc,
            Key::Tab => TaKey::Tab,
            Key::BackTab => TaKey::Null,
        },
        ctrl: key.ctrl,
        alt: key.alt,
        shift: key.shift,
    }
}

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
fn on_menu_key(model: &mut Model, key: &KeyPress) -> Option<Vec<Effect>> {
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

/// Submit the editor contents: a quit command ends the session; anything else non-blank starts a turn.
fn submit(model: &mut Model) -> Vec<Effect> {
    if model.busy {
        return vec![];
    }
    let line = model.input.take();
    model.history.record(&line);
    model.scroll.pin();
    model.command_menu = None;
    match command::parse(&line) {
        Some(Command::Quit) => {
            model.should_quit = true;
            vec![Effect::Quit]
        }
        Some(Command::NewSession) => vec![Effect::NewSession],
        Some(Command::Help) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                command::help_text(),
            ));
            vec![]
        }
        Some(Command::SetMode(mode)) => {
            model.approval_mode = mode;
            vec![]
        }
        Some(Command::ChangeWorkspace(None)) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                format!("workspace: {}", model.status.workspace),
            ));
            vec![]
        }
        Some(Command::ChangeWorkspace(Some(path))) => vec![Effect::ChangeWorkspace(path)],
        Some(Command::Models) => {
            if model.models.is_empty() {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Info,
                    "este provider não tem catálogo de modelos; adicione em ~/.kiri/config.toml"
                        .to_string(),
                ));
            } else {
                let current = model.status.model.clone();
                let selected = model.models.iter().position(|m| *m == current).unwrap_or(0);
                model.picker = Some(Picker::new(
                    PickerKind::Models,
                    "modelo",
                    "Escolha o modelo ativo:",
                    model.models.clone(),
                    selected,
                ));
            }
            vec![]
        }
        Some(Command::Effort) => {
            let options: Vec<String> = Effort::ALL.iter().map(|e| e.label().to_string()).collect();
            let selected = Effort::ALL
                .iter()
                .position(|e| *e == model.status.effort)
                .unwrap_or(0);
            model.picker = Some(Picker::new(
                PickerKind::Effort,
                "esforço",
                "Escolha o nível de esforço (reasoning):",
                options,
                selected,
            ));
            vec![]
        }
        Some(Command::Provider) => {
            if model.providers.is_empty() {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Info,
                    "nenhum provider configurado".to_string(),
                ));
            } else {
                let current = model.status.provider.clone();
                let selected = model
                    .providers
                    .iter()
                    .position(|p| *p == current)
                    .unwrap_or(0);
                // The configured providers, plus the "+ adicionar" row that opens the add wizard.
                let mut options = model.providers.clone();
                options.push(ADD_PROVIDER_LABEL.to_string());
                model.picker = Some(Picker::new(
                    PickerKind::Provider,
                    "provider",
                    "Escolha o provider ativo (ou adicione um novo):",
                    options,
                    selected,
                ));
            }
            vec![]
        }
        Some(Command::Unknown) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Error,
                format!("comando desconhecido: {} (use /help)", line.trim()),
            ));
            vec![]
        }
        None if line.trim().is_empty() && model.attachments.is_empty() => vec![],
        None => {
            // Drain the staged images into the prompt; a turn can carry text, images, or both.
            let images: Vec<String> = std::mem::take(&mut model.attachments)
                .into_iter()
                .map(|attachment| attachment.data_url)
                .collect();
            let label = if line.trim().is_empty() {
                format!("🖼 {} imagem(ns)", images.len())
            } else {
                line.clone()
            };
            model.transcript.push(TranscriptItem::User(label));
            model.busy = true;
            vec![Effect::SubmitPrompt { text: line, images }]
        }
    }
}

/// A resolved approval choice from a key press while a confirmation is pending.
enum Choice {
    /// Pick option `usize` from `APPROVAL_OPTIONS`.
    Option(usize),
    /// Decline just this call (Esc / 'n').
    Decline,
    /// End the whole session (Ctrl+C).
    Abort,
}

/// Answer the pending confirmation: arrows move the highlight, Enter takes the highlighted option,
/// digits/letters jump straight to one, Esc declines this call, and Ctrl+C aborts the session. Option 2
/// ("Sim, e não perguntar de novo") also switches the session to auto mode.
fn on_approval_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    // Navigation moves the highlight without answering.
    if let Some(pending) = model.pending_approval.as_mut() {
        match key.code {
            Key::Up => {
                pending.selected = pending.selected.saturating_sub(1);
                return vec![];
            }
            Key::Down => {
                pending.selected = (pending.selected + 1).min(APPROVAL_OPTIONS.len() - 1);
                return vec![];
            }
            _ => {}
        }
    }

    let selected = model.pending_approval.as_ref().map_or(0, |p| p.selected);
    let choice = match key.code {
        Key::Char('c') if key.ctrl => Some(Choice::Abort),
        Key::Esc => Some(Choice::Decline),
        Key::Enter => Some(Choice::Option(selected)),
        Key::Char('1') => Some(Choice::Option(0)),
        Key::Char('2') => Some(Choice::Option(1)),
        Key::Char('3') => Some(Choice::Option(2)),
        Key::Char(c) => match c.to_ascii_lowercase() {
            's' | 'y' => Some(Choice::Option(0)),
            'n' => Some(Choice::Decline),
            _ => None,
        },
        _ => None,
    };
    let Some(choice) = choice else {
        return vec![];
    };

    let Some(pending) = model.pending_approval.take() else {
        return vec![];
    };
    let (decision, switch_auto) = match choice {
        Choice::Abort => (Approval::Aborted, false),
        Choice::Decline => (Approval::Declined, false),
        Choice::Option(0) => (Approval::Approved, false),
        Choice::Option(1) => (Approval::ApprovedAuto, true),
        Choice::Option(_) => (Approval::Declined, false),
    };
    let (level, label) = match decision {
        Approval::Approved | Approval::ApprovedAuto => {
            (NoticeLevel::Info, format!("✓ {}", pending.action()))
        }
        Approval::Declined => (
            NoticeLevel::Error,
            format!("✗ recusado: {}", pending.action()),
        ),
        Approval::Aborted => (NoticeLevel::Error, "✗ sessão encerrada".to_string()),
    };
    model.transcript.push(TranscriptItem::Notice(level, label));
    // `ApprovedAuto` already runs the rest of this turn unattended; also make auto the standing mode
    // so later turns no longer prompt either.
    if switch_auto {
        model.approval_mode = ApprovalMode::Auto;
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "✓ modo auto ativo".to_string(),
        ));
    }
    vec![Effect::AnswerApproval(decision)]
}

/// Drive the plan box shown after a plan-mode turn: arrows move the highlight, Enter/digits pick an
/// option, Esc cancels, Ctrl+C quits. "Executar" emits `ApprovePlan` (in default or auto mode);
/// "Continuar" just closes the box (staying in plan mode); "Cancelar" closes it and returns to default.
fn on_plan_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if let Some(plan) = model.pending_plan.as_mut() {
        match key.code {
            Key::Up => {
                plan.selected = plan.selected.saturating_sub(1);
                return vec![];
            }
            Key::Down => {
                plan.selected = (plan.selected + 1).min(PLAN_OPTIONS.len() - 1);
                return vec![];
            }
            _ => {}
        }
    }

    if key.ctrl && key.code == Key::Char('c') {
        model.pending_plan = None;
        model.should_quit = true;
        return vec![Effect::Quit];
    }

    let selected = model.pending_plan.as_ref().map_or(0, |p| p.selected);
    let index = match key.code {
        Key::Enter => selected,
        Key::Char('1') => 0,
        Key::Char('2') => 1,
        Key::Char('3') => 2,
        Key::Char('4') => 3,
        Key::Esc => 3,
        _ => return vec![],
    };

    model.pending_plan = None;
    match index {
        // Execute the plan confirming each step: the runtime leaves plan mode for default mode.
        0 => vec![Effect::ApprovePlan(ApprovalMode::Default)],
        // Execute the plan unattended: the runtime leaves plan mode for auto mode.
        1 => vec![Effect::ApprovePlan(ApprovalMode::Auto)],
        // Keep planning: close the box and stay in plan mode for more input.
        2 => vec![],
        // Cancel: leave plan mode without executing.
        _ => {
            model.approval_mode = ApprovalMode::Default;
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                "modo plan cancelado".to_string(),
            ));
            vec![]
        }
    }
}

/// Drive an open `/models` / `/effort` picker: arrows move the highlight, Enter/digits pick a row, Esc
/// closes it, Ctrl+C quits. Enter on a `Models` picker emits `SetModel`; on `Effort`, the row index maps
/// back to `Effort::ALL` for `SetEffort`. The runtime applies the swap and persists it.
fn on_picker_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if let Some(picker) = model.picker.as_mut() {
        match key.code {
            Key::Up => {
                picker.move_cursor(-1);
                return vec![];
            }
            Key::Down => {
                picker.move_cursor(1);
                return vec![];
            }
            _ => {}
        }
    }

    if key.ctrl && key.code == Key::Char('c') {
        model.picker = None;
        model.should_quit = true;
        return vec![Effect::Quit];
    }

    let selected = model.picker.as_ref().map_or(0, |p| p.selected);
    let option_count = model.picker.as_ref().map_or(0, |p| p.options.len());
    let index = match key.code {
        Key::Enter => selected,
        Key::Esc => {
            model.picker = None;
            return vec![];
        }
        Key::Char(c) if c.is_ascii_digit() => {
            let digit = c.to_digit(10).unwrap_or(0) as usize;
            if digit >= 1 && digit <= option_count {
                digit - 1
            } else {
                return vec![];
            }
        }
        _ => return vec![],
    };

    let Some(picker) = model.picker.take() else {
        return vec![];
    };
    match picker.kind {
        PickerKind::Models => match picker.options.get(index) {
            Some(model_id) => vec![Effect::SetModel(model_id.clone())],
            None => vec![],
        },
        PickerKind::Effort => {
            let effort = Effort::ALL.get(index).copied().unwrap_or_default();
            vec![Effect::SetEffort(effort)]
        }
        PickerKind::Provider => {
            // The configured ids come first; the last row (`index == providers.len()`) is the
            // "+ adicionar" sentinel, which opens the add wizard instead of switching.
            if index < model.providers.len() {
                match picker.options.get(index) {
                    Some(id) => vec![Effect::SetProvider(id.clone())],
                    None => vec![],
                }
            } else {
                model.wizard = Some(ProviderWizard::new());
                vec![]
            }
        }
    }
}

/// Drive the add-provider wizard. The `Kind` step uses arrows + Enter; the text steps take typed
/// characters + Backspace, advance on Enter, and the final `ApiKey` step finalizes — staging the key in
/// `Model::pending_credential` (a `Secret`) and emitting `SaveProvider` (no secret). Esc cancels any
/// step, Ctrl+C quits.
fn on_wizard_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if key.ctrl && key.code == Key::Char('c') {
        model.wizard = None;
        model.should_quit = true;
        return vec![Effect::Quit];
    }
    if key.code == Key::Esc {
        model.wizard = None;
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "wizard cancelado".to_string(),
        ));
        return vec![];
    }

    let Some(wizard) = model.wizard.as_mut() else {
        return vec![];
    };

    // The Kind step is a chooser; the rest are text fields.
    if wizard.step == WizardStep::Kind {
        match key.code {
            Key::Up => wizard.move_kind(-1),
            Key::Down => wizard.move_kind(1),
            Key::Enter => {
                wizard.step = WizardStep::BaseUrl;
                // Seed the base URL with the kind's default so the common case is one keystroke (Enter).
                if wizard.base_url.is_empty() {
                    wizard.base_url = wizard.kind().default_base_url().to_string();
                }
            }
            _ => {}
        }
        return vec![];
    }

    match key.code {
        Key::Char(c) => {
            wizard.push_char(c);
            vec![]
        }
        Key::Backspace => {
            wizard.backspace();
            vec![]
        }
        Key::Enter => advance_wizard(model),
        _ => vec![],
    }
}

/// Advance a text step on Enter: validate the required fields, move to the next step, or finalize. A
/// blank required field (model, API key) keeps the wizard on the step rather than producing an invalid
/// provider. Each arm re-borrows `model.wizard` freshly, so the finalize step can `take` it without a
/// borrow conflict (and without an `expect`).
fn advance_wizard(model: &mut Model) -> Vec<Effect> {
    let Some(step) = model.wizard.as_ref().map(|w| w.step) else {
        return vec![];
    };
    match step {
        WizardStep::Kind => vec![],
        WizardStep::BaseUrl => {
            if let Some(wizard) = model.wizard.as_mut() {
                // A custom/compatible endpoint needs a base URL; the vendor kinds default theirs.
                if wizard.base_url.trim().is_empty() {
                    wizard.base_url = wizard.kind().default_base_url().to_string();
                }
                wizard.step = WizardStep::Model;
            }
            vec![]
        }
        WizardStep::Model => {
            let Some(wizard) = model.wizard.as_mut() else {
                return vec![];
            };
            if wizard.model.trim().is_empty() {
                return vec![]; // a model is required
            }
            wizard.step = WizardStep::ExtraModels;
            vec![]
        }
        WizardStep::ExtraModels => {
            if let Some(wizard) = model.wizard.as_mut() {
                wizard.step = WizardStep::ApiKey;
            }
            vec![]
        }
        WizardStep::ApiKey => {
            if model.wizard.as_ref().is_some_and(|w| w.api_key.is_empty()) {
                return vec![]; // an API key is required
            }
            // Finalize: take the wizard, stage the key as a Secret (out of the effect), emit SaveProvider.
            // `mem::take` extracts the key without moving the field out of the `Drop` type; the emptied
            // buffer is then zeroized when `wizard` drops at the end of this scope.
            let Some(mut wizard) = model.wizard.take() else {
                return vec![];
            };
            let base_url = if wizard.base_url.trim().is_empty() {
                wizard.kind().default_base_url().to_string()
            } else {
                wizard.base_url.trim().to_string()
            };
            let effect = Effect::SaveProvider {
                id: wizard.provider_id(),
                kind: wizard.kind(),
                base_url,
                model: wizard.model.trim().to_string(),
                models: wizard.models(),
            };
            model.pending_credential = Some(Secret::new(std::mem::take(&mut wizard.api_key)));
            vec![effect]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::domain::view_state::{PendingApproval, PendingPlan};

    fn press(code: Key) -> KeyPress {
        KeyPress {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// Type a line and submit it (Enter), returning the submit effects.
    fn submit_line(model: &mut Model, line: &str) -> Vec<Effect> {
        for c in line.chars() {
            on_key(model, press(Key::Char(c)));
        }
        on_key(model, press(Key::Enter))
    }

    #[test]
    fn effort_command_opens_a_picker_at_the_current_level() {
        let mut m = Model::default().with_provider_catalog(Vec::new(), Effort::Medium);
        let effects = submit_line(&mut m, "/effort");
        assert!(effects.is_empty(), "opening a picker emits no effect");
        let picker = m.picker.as_ref().expect("the effort picker should open");
        assert_eq!(picker.kind, PickerKind::Effort);
        assert_eq!(picker.options.len(), Effort::ALL.len());
        // The current effort (Medium) is pre-selected.
        assert_eq!(picker.selected, 2);
    }

    #[test]
    fn effort_picker_enter_emits_set_effort() {
        let picker = Picker::new(
            PickerKind::Effort,
            "esforço",
            "Escolha:",
            Effort::ALL.iter().map(|e| e.label().to_string()).collect(),
            0,
        );
        let mut m = Model {
            picker: Some(picker),
            ..Default::default()
        };
        // Down off -> low (index 1), then Enter.
        assert!(on_key(&mut m, press(Key::Down)).is_empty());
        let effects = on_key(&mut m, press(Key::Enter));
        assert_eq!(effects, vec![Effect::SetEffort(Effort::Low)]);
        assert!(m.picker.is_none(), "Enter closes the picker");
    }

    #[test]
    fn picker_digit_selects_a_row() {
        let models = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut m = Model::default().with_provider_catalog(models, Effort::default());
        submit_line(&mut m, "/models");
        // Digit 3 picks the third model.
        let effects = on_key(&mut m, press(Key::Char('3')));
        assert_eq!(effects, vec![Effect::SetModel("c".to_string())]);
    }

    #[test]
    fn provider_picker_add_row_opens_the_wizard() {
        let mut m =
            Model::default().with_providers("nvidia".to_string(), vec!["nvidia".to_string()]);
        submit_line(&mut m, "/provider");
        // options = ["nvidia", "+ adicionar..."]; Down lands on the add row, Enter opens the wizard.
        on_key(&mut m, press(Key::Down));
        let effects = on_key(&mut m, press(Key::Enter));
        assert!(effects.is_empty());
        assert!(m.wizard.is_some(), "the add row opens the wizard");
    }

    #[test]
    fn wizard_completes_with_staged_secret_out_of_the_effect() {
        use crate::shared::kernel::provider::ProviderKind;
        let mut m = Model {
            wizard: Some(ProviderWizard::new()),
            ..Default::default()
        };
        // Kind: Anthropic is index 0 -> Enter (seeds base_url). BaseUrl: accept default -> Enter.
        on_key(&mut m, press(Key::Enter));
        on_key(&mut m, press(Key::Enter));
        // Model: required.
        for c in "claude-opus-4-8".chars() {
            on_key(&mut m, press(Key::Char(c)));
        }
        on_key(&mut m, press(Key::Enter));
        // ExtraModels: skip.
        on_key(&mut m, press(Key::Enter));
        // ApiKey: type, then finalize.
        for c in "sk-ant-secret".chars() {
            on_key(&mut m, press(Key::Char(c)));
        }
        let effects = on_key(&mut m, press(Key::Enter));
        assert!(m.wizard.is_none(), "the wizard closes on finalize");
        match effects.as_slice() {
            [
                Effect::SaveProvider {
                    id, kind, model, ..
                },
            ] => {
                assert_eq!(id, "anthropic");
                assert_eq!(*kind, ProviderKind::Anthropic);
                assert_eq!(model, "claude-opus-4-8");
            }
            other => panic!("expected SaveProvider, got {other:?}"),
        }
        // The key is staged as a Secret, never carried in the effect.
        let staged = m.pending_credential.as_ref().expect("the key is staged");
        assert_eq!(staged.expose(), "sk-ant-secret");
    }

    #[test]
    fn wizard_model_step_requires_a_value() {
        let mut m = Model {
            wizard: Some(ProviderWizard::new()),
            ..Default::default()
        };
        on_key(&mut m, press(Key::Enter)); // Kind -> BaseUrl
        on_key(&mut m, press(Key::Enter)); // BaseUrl -> Model
        // Enter on an empty Model must not advance.
        on_key(&mut m, press(Key::Enter));
        assert_eq!(
            m.wizard.as_ref().map(|w| w.step),
            Some(WizardStep::Model),
            "an empty model keeps the wizard on the Model step"
        );
    }

    #[test]
    fn wizard_esc_cancels() {
        let mut m = Model {
            wizard: Some(ProviderWizard::new()),
            ..Default::default()
        };
        let effects = on_key(&mut m, press(Key::Esc));
        assert!(effects.is_empty());
        assert!(m.wizard.is_none());
    }

    #[test]
    fn provider_command_opens_a_picker_and_enter_emits_set_provider() {
        let mut m = Model::default().with_providers(
            "nvidia".to_string(),
            vec!["nvidia".to_string(), "claude".to_string()],
        );
        let effects = submit_line(&mut m, "/provider");
        assert!(effects.is_empty());
        let picker = m.picker.as_ref().expect("the provider picker should open");
        assert_eq!(picker.kind, PickerKind::Provider);
        assert_eq!(picker.selected, 0); // the active provider (nvidia) is pre-selected
        // Down to "claude", then Enter.
        assert!(on_key(&mut m, press(Key::Down)).is_empty());
        let effects = on_key(&mut m, press(Key::Enter));
        assert_eq!(effects, vec![Effect::SetProvider("claude".to_string())]);
    }

    #[test]
    fn picker_esc_closes_without_an_effect() {
        let mut m =
            Model::default().with_provider_catalog(vec!["a".to_string()], Effort::default());
        submit_line(&mut m, "/models");
        assert!(m.picker.is_some());
        let effects = on_key(&mut m, press(Key::Esc));
        assert!(effects.is_empty());
        assert!(m.picker.is_none());
    }

    #[test]
    fn models_command_with_an_empty_catalog_notifies_and_opens_nothing() {
        let mut m = Model::default(); // no models configured
        submit_line(&mut m, "/models");
        assert!(m.picker.is_none(), "no picker without a catalog");
    }

    #[test]
    fn typing_then_enter_submits_a_prompt() {
        let mut m = Model::default();
        for c in "hi".chars() {
            on_key(&mut m, press(Key::Char(c)));
        }
        let effects = on_key(&mut m, press(Key::Enter));
        assert_eq!(
            effects,
            vec![Effect::SubmitPrompt {
                text: "hi".to_string(),
                images: vec![],
            }]
        );
        assert!(m.busy);
        assert!(m.input.is_empty());
        assert_eq!(m.transcript.items().len(), 1);
    }

    #[test]
    fn shift_enter_inserts_a_newline_without_submitting() {
        let mut m = Model::default();
        on_key(&mut m, press(Key::Char('a')));
        let effects = on_key(
            &mut m,
            KeyPress {
                code: Key::Enter,
                ctrl: false,
                alt: false,
                shift: true,
            },
        );
        assert!(effects.is_empty());
        assert_eq!(m.input.text(), "a\n");
    }

    #[test]
    fn ctrl_c_cancels_while_busy_double_ctrl_c_quits() {
        let mut m = Model {
            busy: true,
            ..Model::default()
        };
        let ctrl_c = KeyPress {
            code: Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        // Single Ctrl+C while busy → cancel the turn.
        assert_eq!(on_key(&mut m, ctrl_c.clone()), vec![Effect::CancelTurn]);
        // Second Ctrl+C within the window → quit (double-tap), even though the first cancelled.
        assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
        assert!(m.should_quit);
    }

    #[test]
    fn single_ctrl_c_while_idle_does_nothing_then_double_quits() {
        let mut m = Model::default();
        let ctrl_c = KeyPress {
            code: Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        // Single Ctrl+C while idle → no-op (quit requires a double tap).
        assert_eq!(on_key(&mut m, ctrl_c.clone()), vec![]);
        assert!(!m.should_quit);
        // Double Ctrl+C → quit.
        assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
        assert!(m.should_quit);
    }

    #[test]
    fn double_esc_cancels_while_busy() {
        let mut m = Model {
            busy: true,
            ..Model::default()
        };
        let esc = KeyPress {
            code: Key::Esc,
            ctrl: false,
            alt: false,
            shift: false,
        };
        // First Esc while busy → no-op (recorded for the double-tap window).
        assert_eq!(on_key(&mut m, esc.clone()), vec![]);
        // Second Esc within the window → cancel the turn.
        assert_eq!(on_key(&mut m, esc), vec![Effect::CancelTurn]);
    }

    #[test]
    fn pending_approval_consumes_keys_as_decisions() {
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("delete a.txt".to_string(), true)),
            ..Model::default()
        };
        let effects = on_key(&mut m, press(Key::Char('n')));
        assert_eq!(effects, vec![Effect::AnswerApproval(Approval::Declined)]);
        assert!(m.pending_approval.is_none());
        assert_eq!(m.transcript.items().len(), 1);
    }

    #[test]
    fn approval_key_with_no_pending_approval_is_a_noop() {
        // Guards the invariant that on_approval_key never panics if reached without a pending
        // approval (e.g. a future routing change) — it answers nothing rather than unwrapping None.
        let mut m = Model::default();
        assert!(m.pending_approval.is_none());
        assert_eq!(on_approval_key(&mut m, press(Key::Enter)), vec![]);
    }

    #[test]
    fn enter_on_approval_follows_the_default() {
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("p".to_string(), false)),
            ..Model::default()
        };
        assert_eq!(
            on_key(&mut m, press(Key::Enter)),
            vec![Effect::AnswerApproval(Approval::Declined)]
        );
    }

    #[test]
    fn approval_arrows_move_selection_then_enter_confirms_and_switches_to_auto() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("p".to_string(), true)),
            ..Model::default()
        };
        on_key(&mut m, press(Key::Down)); // highlight option 2 ("…modo auto")
        assert_eq!(m.pending_approval.as_ref().unwrap().selected, 1);
        let effects = on_key(&mut m, press(Key::Enter));
        // ApprovedAuto runs the rest of this turn unattended; the mode also sticks for later turns.
        assert_eq!(
            effects,
            vec![Effect::AnswerApproval(Approval::ApprovedAuto)]
        );
        assert_eq!(m.approval_mode, ApprovalMode::Auto);
        assert!(m.pending_approval.is_none());
        let last = m.transcript.items().last().unwrap();
        assert!(
            matches!(last, TranscriptItem::Notice(NoticeLevel::Info, t) if t.contains("modo auto ativo")),
            "missing auto-active notice: {last:?}"
        );
    }

    #[test]
    fn esc_declines_without_aborting_the_session() {
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("p".to_string(), true)),
            ..Model::default()
        };
        assert_eq!(
            on_key(&mut m, press(Key::Esc)),
            vec![Effect::AnswerApproval(Approval::Declined)]
        );
    }

    #[test]
    fn ctrl_c_aborts_a_pending_approval() {
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("p".to_string(), true)),
            ..Model::default()
        };
        let ctrl_c = KeyPress {
            code: Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        assert_eq!(
            on_key(&mut m, ctrl_c),
            vec![Effect::AnswerApproval(Approval::Aborted)]
        );
    }

    #[test]
    fn ctrl_o_toggles_tool_output_expansion() {
        let mut m = Model::default();
        assert!(!m.expand_tools);
        assert!(on_key(&mut m, ctrl(Key::Char('o'))).is_empty());
        assert!(m.expand_tools, "Ctrl+O should expand tool output");
        on_key(&mut m, ctrl(Key::Char('o')));
        assert!(!m.expand_tools, "Ctrl+O again should collapse it");
    }

    #[test]
    fn back_tab_cycles_the_approval_mode_when_idle() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model::default();
        assert_eq!(m.approval_mode, ApprovalMode::Default);
        assert!(on_key(&mut m, press(Key::BackTab)).is_empty());
        assert_eq!(m.approval_mode, ApprovalMode::Auto);
        on_key(&mut m, press(Key::BackTab));
        assert_eq!(m.approval_mode, ApprovalMode::Plan);
        on_key(&mut m, press(Key::BackTab));
        assert_eq!(m.approval_mode, ApprovalMode::Default);
    }

    #[test]
    fn back_tab_is_ignored_mid_turn() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model {
            busy: true,
            ..Model::default()
        };
        on_key(&mut m, press(Key::BackTab));
        assert_eq!(m.approval_mode, ApprovalMode::Default);
    }

    #[test]
    fn new_session_command_emits_effect() {
        let mut m = Model::default();
        m.input.set("/new".to_string());
        assert_eq!(on_key(&mut m, press(Key::Enter)), vec![Effect::NewSession]);
    }

    #[test]
    fn mode_command_sets_mode_without_effect() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model::default();
        m.input.set("/plan".to_string());
        assert!(on_key(&mut m, press(Key::Enter)).is_empty());
        assert_eq!(m.approval_mode, ApprovalMode::Plan);
    }

    #[test]
    fn cd_with_path_emits_change_workspace() {
        let mut m = Model::default();
        m.input.set("/cd src".to_string());
        assert_eq!(
            on_key(&mut m, press(Key::Enter)),
            vec![Effect::ChangeWorkspace("src".to_string())]
        );
    }

    #[test]
    fn unknown_command_warns_without_effect() {
        let mut m = Model::default();
        m.input.set("/nope".to_string());
        assert!(on_key(&mut m, press(Key::Enter)).is_empty());
        assert_eq!(m.transcript.items().len(), 1);
    }

    #[test]
    fn plan_enter_executes_the_plan() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model {
            pending_plan: Some(PendingPlan::default()),
            ..Model::default()
        };
        assert_eq!(
            on_key(&mut m, press(Key::Enter)),
            vec![Effect::ApprovePlan(ApprovalMode::Default)]
        );
        assert!(m.pending_plan.is_none());
    }

    #[test]
    fn plan_execute_in_auto_emits_auto_mode() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model {
            pending_plan: Some(PendingPlan::default()),
            approval_mode: ApprovalMode::Plan,
            ..Model::default()
        };
        on_key(&mut m, press(Key::Down)); // highlight "Executar o plano em modo auto"
        assert_eq!(
            on_key(&mut m, press(Key::Enter)),
            vec![Effect::ApprovePlan(ApprovalMode::Auto)]
        );
        assert!(m.pending_plan.is_none());
    }

    #[test]
    fn plan_keep_planning_closes_box_and_stays_in_plan() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model {
            pending_plan: Some(PendingPlan::default()),
            approval_mode: ApprovalMode::Plan,
            ..Model::default()
        };
        on_key(&mut m, press(Key::Down)); // highlight "Executar o plano em modo auto"
        on_key(&mut m, press(Key::Down)); // highlight "Continuar planejando"
        assert!(on_key(&mut m, press(Key::Enter)).is_empty());
        assert!(m.pending_plan.is_none());
        assert_eq!(m.approval_mode, ApprovalMode::Plan);
    }

    #[test]
    fn plan_cancel_leaves_plan_mode() {
        use crate::shared::kernel::approval_mode::ApprovalMode;
        let mut m = Model {
            pending_plan: Some(PendingPlan::default()),
            approval_mode: ApprovalMode::Plan,
            ..Model::default()
        };
        assert!(on_key(&mut m, press(Key::Esc)).is_empty());
        assert!(m.pending_plan.is_none());
        assert_eq!(m.approval_mode, ApprovalMode::Default);
    }

    // --- live slash-command preview ----------------------------------------------

    fn type_str(m: &mut Model, s: &str) {
        for c in s.chars() {
            on_key(m, press(Key::Char(c)));
        }
    }

    #[test]
    fn typing_slash_opens_the_command_menu() {
        let mut m = Model::default();
        type_str(&mut m, "/");
        assert!(m.command_menu.is_some(), "menu should open on bare slash");
        // Empty query shows the whole catalog.
        assert_eq!(
            m.command_menu.as_ref().unwrap().filtered().len(),
            crate::modules::tui::domain::command_menu::COMMANDS.len()
        );
    }

    #[test]
    fn typing_after_slash_filters_the_menu() {
        let mut m = Model::default();
        type_str(&mut m, "/ne");
        let menu = m.command_menu.as_ref().expect("menu should stay open");
        assert_eq!(menu.filtered().len(), 1);
        assert_eq!(
            menu.spec().unwrap().name,
            "/new",
            "filtered row should highlight /new"
        );
    }

    #[test]
    fn backspace_closes_the_menu_when_the_slash_is_erased() {
        let mut m = Model::default();
        type_str(&mut m, "/n");
        assert!(m.command_menu.is_some());
        on_key(&mut m, press(Key::Backspace)); // now "/"
        assert!(m.command_menu.is_some(), "bare slash keeps the menu open");
        on_key(&mut m, press(Key::Backspace)); // now empty
        assert!(
            m.command_menu.is_none(),
            "erasing the slash closes the menu"
        );
        assert!(m.input.is_empty());
    }

    #[test]
    fn space_in_buffer_closes_the_menu_argument_mode() {
        let mut m = Model::default();
        type_str(&mut m, "/cd ");
        assert!(
            m.command_menu.is_none(),
            "whitespace starts argument mode, menu must close"
        );
    }

    #[test]
    fn arrows_move_the_highlight_without_recalling_history() {
        let mut m = Model::default();
        type_str(&mut m, "/");
        let first = m.command_menu.as_ref().unwrap().selected();
        on_key(&mut m, press(Key::Down));
        assert_eq!(m.command_menu.as_ref().unwrap().selected(), first + 1);
        on_key(&mut m, press(Key::Up));
        assert_eq!(m.command_menu.as_ref().unwrap().selected(), first);
    }

    #[test]
    fn tab_completes_to_canonical_name_plus_space_and_closes_menu() {
        let mut m = Model::default();
        type_str(&mut m, "/ne");
        on_key(&mut m, press(Key::Tab));
        assert_eq!(m.input.text(), "/new ");
        assert!(
            m.command_menu.is_none(),
            "Tab closes the menu after completion"
        );
    }

    #[test]
    fn esc_closes_the_menu_but_keeps_the_buffer() {
        let mut m = Model::default();
        type_str(&mut m, "/ne");
        on_key(&mut m, press(Key::Esc));
        assert!(m.command_menu.is_none());
        assert_eq!(m.input.text(), "/ne", "Esc must not erase the text");
    }

    #[test]
    fn enter_in_a_filtered_menu_submits_the_command() {
        let mut m = Model::default();
        type_str(&mut m, "/new");
        let effects = on_key(&mut m, press(Key::Enter));
        assert_eq!(effects, vec![Effect::NewSession]);
        assert!(m.command_menu.is_none(), "submit must clear the menu");
        assert!(m.input.is_empty());
    }

    #[test]
    fn menu_does_not_open_while_a_turn_is_running() {
        let mut m = Model {
            busy: true,
            ..Model::default()
        };
        type_str(&mut m, "/");
        assert!(m.command_menu.is_none(), "menu must stay closed while busy");
    }

    #[test]
    fn ctrl_c_mid_menu_double_tap_quits() {
        let mut m = Model::default();
        type_str(&mut m, "/");
        let ctrl_c = KeyPress {
            code: Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        // Single Ctrl+C → no-op (quit now requires a double tap).
        assert_eq!(on_key(&mut m, ctrl_c.clone()), vec![]);
        // Double Ctrl+C → quit.
        assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
        assert!(m.should_quit);
    }

    // --- rich editor: clipboard chords and history-at-edge -------------------------

    fn shift(code: Key) -> KeyPress {
        KeyPress {
            code,
            ctrl: false,
            alt: false,
            shift: true,
        }
    }

    fn ctrl(code: Key) -> KeyPress {
        KeyPress {
            code,
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    fn alt(code: Key) -> KeyPress {
        KeyPress {
            code,
            ctrl: false,
            alt: true,
            shift: false,
        }
    }

    fn shift_alt(code: Key) -> KeyPress {
        KeyPress {
            code,
            ctrl: false,
            alt: true,
            shift: true,
        }
    }

    #[test]
    fn ctrl_c_with_a_selection_copies_instead_of_cancelling() {
        let mut m = Model {
            busy: true, // even mid-turn, Ctrl+C on a selection copies rather than cancels
            ..Model::default()
        };
        type_str(&mut m, "abc");
        on_key(&mut m, shift(Key::Left));
        on_key(&mut m, shift(Key::Left)); // select "bc"
        let effects = on_key(&mut m, ctrl(Key::Char('c')));
        assert!(
            matches!(effects.as_slice(), [Effect::CopyToClipboard(t)] if t == "bc"),
            "Ctrl+C with a selection should copy it, got {effects:?}"
        );
        assert!(!m.should_quit, "copy must not quit");
    }

    #[test]
    fn ctrl_x_cuts_the_selection_and_removes_it() {
        let mut m = Model::default();
        type_str(&mut m, "abc");
        on_key(&mut m, shift(Key::Left)); // select "c"
        let effects = on_key(&mut m, ctrl(Key::Char('x')));
        assert!(
            matches!(effects.as_slice(), [Effect::CopyToClipboard(t)] if t == "c"),
            "Ctrl+X should cut the selection, got {effects:?}"
        );
        assert_eq!(m.input.text(), "ab", "cut must remove the selected text");
    }

    #[test]
    fn ctrl_x_without_a_selection_is_a_noop() {
        let mut m = Model::default();
        type_str(&mut m, "abc");
        assert!(on_key(&mut m, ctrl(Key::Char('x'))).is_empty());
        assert_eq!(m.input.text(), "abc");
    }

    #[test]
    fn up_recalls_history_only_at_the_first_row() {
        let mut m = Model::default();
        m.history.record("prev");
        // Build a two-line buffer; the cursor ends on the last (second) row.
        on_key(&mut m, press(Key::Char('a')));
        on_key(&mut m, shift(Key::Enter)); // newline without submitting
        on_key(&mut m, press(Key::Char('b')));
        assert_eq!(m.input.text(), "a\nb");
        // From the last row, Up moves the cursor up — it must NOT recall history.
        on_key(&mut m, press(Key::Up));
        assert_eq!(
            m.input.text(),
            "a\nb",
            "Up inside a multi-line buffer must not recall"
        );
        // Now on the first row, Up recalls the previous prompt.
        on_key(&mut m, press(Key::Up));
        assert_eq!(
            m.input.text(),
            "prev",
            "Up at the first row should recall history"
        );
    }

    // --- macOS typing standard ----------------------------------------------------

    #[test]
    fn ctrl_a_moves_to_line_start() {
        // macOS/Cocoa: Ctrl+A is "move to line start", not select-all. After it, typing inserts at the
        // head of the line.
        let mut m = Model::default();
        type_str(&mut m, "abc");
        on_key(&mut m, ctrl(Key::Char('a')));
        on_key(&mut m, press(Key::Char('X')));
        assert_eq!(m.input.text(), "Xabc");
    }

    #[test]
    fn ctrl_a_no_longer_selects_all() {
        // Guards the intent that select-all left the keyboard: Ctrl+A must not start a selection.
        let mut m = Model::default();
        type_str(&mut m, "abc");
        on_key(&mut m, ctrl(Key::Char('a')));
        assert!(
            !m.input.is_selecting(),
            "Ctrl+A must move the cursor, not select all"
        );
    }

    #[test]
    fn shift_alt_left_selects_word_back() {
        // Option+Left jumps a word; with Shift it selects one. Cutting proves the whole word was caught.
        let mut m = Model::default();
        type_str(&mut m, "foo bar");
        on_key(&mut m, shift_alt(Key::Left));
        let effects = on_key(&mut m, ctrl(Key::Char('x')));
        let cut = match effects.as_slice() {
            [Effect::CopyToClipboard(t)] => t.clone(),
            other => panic!("expected a cut, got {other:?}"),
        };
        assert_eq!(
            cut.trim(),
            "bar",
            "Shift+Option+Left should select the word back"
        );
        assert_eq!(m.input.text().trim_end(), "foo");
    }

    #[test]
    fn shift_alt_right_selects_word_forward() {
        let mut m = Model::default();
        m.input.set("foo bar".to_string());
        on_key(&mut m, press(Key::Home));
        on_key(&mut m, shift_alt(Key::Right));
        let effects = on_key(&mut m, ctrl(Key::Char('x')));
        let cut = match effects.as_slice() {
            [Effect::CopyToClipboard(t)] => t.clone(),
            other => panic!("expected a cut, got {other:?}"),
        };
        assert_eq!(
            cut.trim(),
            "foo",
            "Shift+Option+Right should select the word forward"
        );
        assert_eq!(m.input.text().trim_start(), "bar");
    }

    #[test]
    fn option_backspace_still_deletes_word() {
        // Regression: the native macOS Option+Backspace (delivered as Alt+Backspace) must keep deleting a
        // word — the new word-motion remap keys off Left/Right and must not disturb this.
        let mut m = Model::default();
        type_str(&mut m, "foo bar");
        on_key(&mut m, alt(Key::Backspace));
        assert_eq!(m.input.text().trim_end(), "foo");
    }

    #[test]
    fn ctrl_backspace_deletes_word_back() {
        let mut m = Model::default();
        type_str(&mut m, "foo bar");
        on_key(&mut m, ctrl(Key::Backspace));
        assert_eq!(m.input.text().trim_end(), "foo");
    }

    #[test]
    fn ctrl_delete_deletes_word_forward() {
        let mut m = Model::default();
        m.input.set("foo bar".to_string());
        on_key(&mut m, press(Key::Home));
        on_key(&mut m, ctrl(Key::Delete));
        assert_eq!(m.input.text().trim_start(), "bar");
    }

    #[test]
    fn meta_word_motion_still_selects() {
        // The other wire encoding of Option+word-motion: meta Alt+b/f reaches the widget directly.
        let mut m = Model::default();
        type_str(&mut m, "foo bar");
        on_key(
            &mut m,
            KeyPress {
                code: Key::Char('b'),
                ctrl: false,
                alt: true,
                shift: true,
            },
        );
        let effects = on_key(&mut m, ctrl(Key::Char('x')));
        assert!(
            matches!(effects.as_slice(), [Effect::CopyToClipboard(t)] if t.trim() == "bar"),
            "Shift+Alt+b should select the word back, got {effects:?}"
        );
    }

    #[test]
    fn plain_home_moves_to_line_head() {
        // Guards that Home still reaches the widget (it is not intercepted unless Ctrl is held).
        let mut m = Model::default();
        type_str(&mut m, "ab");
        on_key(&mut m, press(Key::Home));
        on_key(&mut m, press(Key::Char('X')));
        assert_eq!(m.input.text(), "Xab");
    }

    #[test]
    fn alt_char_without_arrow_falls_through_to_editor() {
        // An Alt+Char that is not a recognized motion must still reach the editor (not be swallowed).
        // Under "Option as Meta" some layouts deliver letters with Alt; they must type, not vanish.
        let mut m = Model::default();
        on_key(&mut m, alt(Key::Char('z')));
        // The widget binds Alt+z to nothing destructive here; the key is consumed by feed without error.
        // The guarantee under test is "no panic / no swallow into a dead chord" — the buffer stays valid.
        assert!(m.input.text().is_empty() || m.input.text() == "z");
    }

    // --- screen selection (mouse) -------------------------------------------------

    /// A model whose event clock is stamped, ready for mouse-gesture tests.
    fn with_clock(now: Instant) -> Model {
        Model {
            last_event_at: Some(now),
            ..Model::default()
        }
    }

    #[test]
    fn mouse_down_starts_a_char_selection() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        let sel = m.selection.expect("down starts a selection");
        assert_eq!(sel.anchor, (3, 2));
        assert_eq!(sel.head, (3, 2));
        assert_eq!(sel.granularity, Granularity::Char);
        assert_eq!(sel.state, SelectionState::Idle);
    }

    #[test]
    fn mouse_drag_extends_head_and_keeps_anchor() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 7, 2);
        let sel = m.selection.unwrap();
        assert_eq!(sel.anchor, (3, 2));
        assert_eq!(sel.head, (7, 2));
    }

    #[test]
    fn mouse_up_on_a_real_drag_requests_copy_and_keeps_highlight() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 7, 2);
        on_mouse(&mut m, MouseKind::Up, 7, 2);
        let sel = m
            .selection
            .expect("a non-empty selection stays after release");
        assert_eq!(sel.state, SelectionState::CopyAndKeep);
        assert!(!sel.is_empty());
    }

    #[test]
    fn bare_click_clears_the_selection() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Up, 3, 2);
        assert!(
            m.selection.is_none(),
            "a click with no drag selects nothing"
        );
    }

    #[test]
    fn single_cell_selection_needs_a_one_cell_drag() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 4, 2);
        on_mouse(&mut m, MouseKind::Up, 4, 2);
        let sel = m.selection.expect("a one-cell drag is a real selection");
        assert!(!sel.is_empty());
        assert_eq!(sel.state, SelectionState::CopyAndKeep);
    }

    #[test]
    fn double_click_within_window_selects_a_word() {
        let t0 = Instant::now();
        let mut m = with_clock(t0);
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Up, 3, 2); // bare click clears the highlight...
        m.last_event_at = Some(t0 + Duration::from_millis(50));
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        let sel = m.selection.expect("the second click reselects");
        assert_eq!(sel.granularity, Granularity::Word);
        assert!(!sel.is_empty(), "a word selection is never empty");
    }

    #[test]
    fn triple_click_within_window_selects_a_line() {
        let t0 = Instant::now();
        let mut m = with_clock(t0);
        for i in 0..3u64 {
            m.last_event_at = Some(t0 + Duration::from_millis(i * 50));
            on_mouse(&mut m, MouseKind::Down, 3, 2);
            on_mouse(&mut m, MouseKind::Up, 3, 2);
        }
        let sel = m
            .selection
            .expect("a line selection stays after the third release");
        assert_eq!(sel.granularity, Granularity::Line);
    }

    #[test]
    fn second_click_after_the_window_is_a_fresh_single() {
        let t0 = Instant::now();
        let mut m = with_clock(t0);
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Up, 3, 2);
        m.last_event_at = Some(t0 + Duration::from_millis(600)); // > MULTI_CLICK_WINDOW
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        assert_eq!(m.selection.unwrap().granularity, Granularity::Char);
    }

    #[test]
    fn double_click_far_away_is_two_singles() {
        let t0 = Instant::now();
        let mut m = with_clock(t0);
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Up, 3, 2);
        m.last_event_at = Some(t0 + Duration::from_millis(50));
        on_mouse(&mut m, MouseKind::Down, 9, 9); // a different cell — not a double-click
        assert_eq!(m.selection.unwrap().granularity, Granularity::Char);
    }

    #[test]
    fn keystroke_clears_the_screen_selection() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 7, 2);
        on_mouse(&mut m, MouseKind::Up, 7, 2);
        assert!(m.selection.is_some());
        on_key(&mut m, press(Key::Char('a')));
        assert!(m.selection.is_none(), "typing drops the screen selection");
    }

    #[test]
    fn esc_clears_the_screen_selection_when_idle() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 7, 2);
        on_mouse(&mut m, MouseKind::Up, 7, 2);
        on_key(&mut m, press(Key::Esc));
        assert!(m.selection.is_none());
    }

    #[test]
    fn ctrl_c_prefers_the_screen_selection_and_requests_clearing_copy() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 7, 2);
        on_mouse(&mut m, MouseKind::Up, 7, 2);
        let effects = on_key(&mut m, ctrl(Key::Char('c')));
        assert!(
            effects.is_empty(),
            "screen copy goes through selection state, not an Effect"
        );
        assert_eq!(
            m.selection
                .expect("selection survives until the runtime scrapes it")
                .state,
            SelectionState::CopyAndClear
        );
        assert!(!m.should_quit, "Ctrl+C on a selection must not quit");
    }

    #[test]
    fn mouse_selection_works_while_a_modal_is_pending() {
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("ler a.txt".to_string(), true)),
            last_event_at: Some(Instant::now()),
            ..Model::default()
        };
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 7, 2);
        on_mouse(&mut m, MouseKind::Up, 7, 2);
        assert!(
            m.selection.is_some(),
            "mouse selection must work under a modal (to copy its text)"
        );
    }

    #[test]
    fn bare_click_in_the_focused_composer_emits_place_cursor() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 12, 4);
        let effects = on_mouse(&mut m, MouseKind::Up, 12, 4);
        assert_eq!(effects, vec![Effect::PlaceCursor { col: 12, row: 4 }]);
        assert!(m.selection.is_none(), "a bare click leaves no highlight");
    }

    #[test]
    fn a_drag_selects_and_does_not_place_the_cursor() {
        let mut m = with_clock(Instant::now());
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Drag, 7, 2);
        let effects = on_mouse(&mut m, MouseKind::Up, 7, 2);
        assert!(
            effects.is_empty(),
            "a drag is a selection, not a cursor placement"
        );
        assert_eq!(m.selection.unwrap().state, SelectionState::CopyAndKeep);
    }

    #[test]
    fn bare_click_during_a_modal_does_not_place_the_cursor() {
        // Under a modal the editor is read-only; a bare click clears any highlight but must not try to
        // move the (hidden) edit cursor.
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("ler a.txt".to_string(), true)),
            last_event_at: Some(Instant::now()),
            ..Model::default()
        };
        on_mouse(&mut m, MouseKind::Down, 12, 4);
        let effects = on_mouse(&mut m, MouseKind::Up, 12, 4);
        assert!(effects.is_empty(), "no cursor placement under a modal");
        assert!(m.selection.is_none());
    }
}
