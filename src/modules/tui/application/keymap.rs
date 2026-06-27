use std::time::{Duration, Instant};

use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Key, KeyPress, MouseKind};
use crate::modules::tui::domain::command_menu::CommandMenu;
use crate::modules::tui::domain::modal::{APPROVAL_OPTIONS, PLAN_OPTIONS};
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::picker::{Picker, PickerKind};
use crate::modules::tui::domain::selection::{Granularity, ScreenSelection, SelectionState};
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::domain::wizard::{ADD_PROVIDER_LABEL, ProviderWizard, WizardStep};
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::provider::{AuthMethod, Effort, Secret};
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
        Some(Command::Resume) => vec![Effect::ResumeLast],
        Some(Command::Sessions) => vec![Effect::ListSessions],
        Some(Command::Sync) => vec![Effect::SyncPush],
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
        Some(Command::Models) => open_models_picker(model),
        Some(Command::Effort) => open_effort_picker(model),
        Some(Command::Provider) => open_provider_picker(model),
        Some(Command::Unknown) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Error,
                format!("comando desconhecido: {} (use /help)", line.trim()),
            ));
            vec![]
        }
        None if line.trim().is_empty() && model.attachments.is_empty() => vec![],
        None if model.unconfigured => {
            // No usable provider yet: never send to the null provider silently. Drop the staged images,
            // surface a clear notice, and re-open onboarding. `busy` is intentionally left false so no
            // turn is armed and the UI is not stranded.
            model.attachments.clear();
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                "configure um provider primeiro — escolha um e informe a API key".to_string(),
            ));
            model.wizard = Some(ProviderWizard::onboarding());
            vec![]
        }
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

/// Open the `/models` picker for the active provider's catalog, preselecting the current model. An empty
/// catalog surfaces a notice instead — there is nothing to pick.
fn open_models_picker(model: &mut Model) -> Vec<Effect> {
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

/// Open the `/effort` picker over the reasoning-effort levels, preselecting the current effort.
fn open_effort_picker(model: &mut Model) -> Vec<Effect> {
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

/// Open the `/provider` picker over the configured providers (plus the "+ adicionar" row that opens the
/// add wizard), preselecting the active one. With no providers configured it surfaces a notice instead.
fn open_provider_picker(model: &mut Model) -> Vec<Effect> {
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
        PickerKind::Sessions => match model.session_ids.get(index) {
            // The labels are display titles; the id comes from the parallel `session_ids`, filled by
            // the runtime when it opened the picker.
            Some(id) => vec![Effect::OpenSession(id.clone())],
            None => vec![],
        },
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
        // Cancelling onboarding must not strand a credential-less app: keep the submit gate up and post a
        // persistent hint. The next stray prompt re-opens onboarding (via the gate), and /provider works.
        let onboarding = model.wizard.as_ref().is_some_and(|w| w.onboarding);
        model.wizard = None;
        let message = if onboarding {
            "configure um provider com /provider para começar"
        } else {
            "wizard cancelado"
        };
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            message.to_string(),
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
                if wizard.key_required() {
                    // Vendor kinds use a canonical id; go straight to the base URL, seeded with the
                    // kind's default so the common case is one keystroke (Enter).
                    wizard.step = WizardStep::BaseUrl;
                    if wizard.base_url.is_empty() {
                        wizard.base_url = wizard.kind().default_base_url().to_string();
                    }
                } else {
                    // Keyless-capable kinds let the user name the provider so several can coexist; seed
                    // the field with the canonical token as an editable suggestion.
                    wizard.step = WizardStep::ProviderId;
                    if wizard.id.is_empty() {
                        wizard.id = wizard.provider_id();
                    }
                }
            }
            _ => {}
        }
        return vec![];
    }

    match key.code {
        // Ctrl+V pastes into the masked field via the clipboard (which routes to the wizard, never the
        // plaintext composer), instead of inserting a literal 'v'. Critical on the API-key step: a
        // pasted key would otherwise be silently corrupted, and the field is masked so it is invisible.
        Key::Char('v') if key.ctrl && !key.alt => vec![Effect::PasteClipboard],
        // Only a plain character types into the field; any other chord is ignored so it cannot corrupt
        // the input (e.g. Ctrl+A inserting 'a').
        Key::Char(c) if !key.ctrl && !key.alt => {
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
        WizardStep::ProviderId => {
            if let Some(wizard) = model.wizard.as_mut() {
                // The id is sanitized at finalize (provider_id) and falls back to the canonical token, so
                // a blank field is acceptable here; seed the base URL for the next step.
                wizard.step = WizardStep::BaseUrl;
                if wizard.base_url.trim().is_empty() {
                    wizard.base_url = wizard.kind().default_base_url().to_string();
                }
            }
            vec![]
        }
        WizardStep::BaseUrl => {
            let Some(wizard) = model.wizard.as_mut() else {
                return vec![];
            };
            if wizard.base_url.trim().is_empty() {
                // Vendor kinds default their endpoint; compatible/custom have none, so a blank base URL
                // stays on the step rather than saving an unusable endpoint that POSTs to "/chat/completions".
                let default = wizard.kind().default_base_url();
                if default.is_empty() {
                    return vec![]; // a base URL is required for compatible/custom
                }
                wizard.base_url = default.to_string();
            }
            wizard.step = WizardStep::Model;
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
            // The key is optional for keyless-capable kinds and required for vendor kinds; its presence
            // decides the auth method, so the two can never disagree.
            let has_key = model
                .wizard
                .as_ref()
                .is_some_and(|w| !w.api_key.trim().is_empty());
            let key_required = model.wizard.as_ref().is_some_and(|w| w.key_required());
            if !has_key && key_required {
                return vec![]; // a vendor kind requires a key
            }
            // Finalize: take the wizard, stage the key as a Secret (out of the effect) only when present,
            // emit SaveProvider. `mem::take` extracts the key without moving the field out of the `Drop`
            // type; the emptied buffer is then zeroized when `wizard` drops at the end of this scope.
            let Some(mut wizard) = model.wizard.take() else {
                return vec![];
            };
            let base_url = if wizard.base_url.trim().is_empty() {
                wizard.kind().default_base_url().to_string()
            } else {
                wizard.base_url.trim().to_string()
            };
            let auth = if has_key {
                AuthMethod::ApiKey
            } else {
                AuthMethod::None
            };
            let effect = Effect::SaveProvider {
                id: wizard.provider_id(),
                kind: wizard.kind(),
                base_url,
                model: wizard.model.trim().to_string(),
                models: wizard.models(),
                auth,
            };
            if has_key {
                model.pending_credential = Some(Secret::new(std::mem::take(&mut wizard.api_key)));
            }
            vec![effect]
        }
    }
}

#[cfg(test)]
mod tests;
