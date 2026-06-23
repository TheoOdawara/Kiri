use crate::modules::agent::application::approval_policy::{Approval, ApprovalMode};
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Key, KeyPress};
use crate::modules::tui::domain::command_menu::CommandMenu;
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::domain::view_state::{APPROVAL_OPTIONS, PLAN_OPTIONS};

const SCROLL_STEP: u16 = 5;

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

    // Before anything else: the menu intercepts navigation and completion keys, but lets ordinary
    // typing fall through to the editor (which keeps the filter in sync after each mutation).
    if model.command_menu.is_some()
        && let Some(effects) = on_menu_key(model, &key)
    {
        return effects;
    }

    // Control chords take precedence over text input.
    if key.ctrl {
        match key.code {
            Key::Char('c') => {
                return if model.busy {
                    vec![Effect::CancelTurn]
                } else {
                    model.should_quit = true;
                    vec![Effect::Quit]
                };
            }
            Key::Char('d') if model.input.is_empty() => {
                model.should_quit = true;
                return vec![Effect::Quit];
            }
            _ => {}
        }
    }

    match key.code {
        Key::Enter if key.shift || key.alt => {
            model.input.insert_char('\n');
            sync_menu(model);
            vec![]
        }
        Key::Enter => submit(model),
        Key::Char(c) if !key.ctrl => {
            model.input.insert_char(c);
            sync_menu(model);
            vec![]
        }
        Key::Backspace => {
            model.input.backspace();
            sync_menu(model);
            vec![]
        }
        Key::Delete => {
            model.input.delete();
            sync_menu(model);
            vec![]
        }
        Key::Left => {
            model.input.left();
            sync_menu(model);
            vec![]
        }
        Key::Right => {
            model.input.right();
            sync_menu(model);
            vec![]
        }
        Key::Home => {
            model.input.home();
            sync_menu(model);
            vec![]
        }
        Key::End => {
            model.input.end();
            sync_menu(model);
            vec![]
        }
        Key::Up => {
            if let Some(line) = model.history.older(model.input.text()) {
                model.input.set(line);
                sync_menu(model);
            }
            vec![]
        }
        Key::Down => {
            if let Some(line) = model.history.newer() {
                model.input.set(line);
                sync_menu(model);
            }
            vec![]
        }
        Key::PageUp => {
            model.scroll.up(SCROLL_STEP);
            vec![]
        }
        Key::PageDown => {
            model.scroll.down(SCROLL_STEP);
            vec![]
        }
        // Shift+Tab cycles the approval mode (Default -> Auto -> Plan); ignored mid-turn, since the
        // mode is read when a turn starts.
        Key::BackTab => {
            if !model.busy {
                model.approval_mode = model.approval_mode.next();
            }
            vec![]
        }
        _ => vec![],
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
        && text.starts_with('/')
        && !text.chars().any(char::is_whitespace);
    if !can_open {
        model.command_menu = None;
        return;
    }
    match &mut model.command_menu {
        Some(menu) => menu.refresh(text),
        slot @ None => *slot = Some(CommandMenu::open(text)),
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
        Some(Command::Unknown) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Error,
                format!("comando desconhecido: {} (use /help)", line.trim()),
            ));
            vec![]
        }
        None if line.trim().is_empty() => vec![],
        None => {
            model.transcript.push(TranscriptItem::User(line.clone()));
            model.busy = true;
            vec![Effect::SubmitPrompt(line)]
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

    let pending = model
        .pending_approval
        .take()
        .expect("an approval is pending");
    let (decision, switch_auto) = match choice {
        Choice::Abort => (Approval::Aborted, false),
        Choice::Decline => (Approval::Declined, false),
        Choice::Option(0) => (Approval::Approved, false),
        Choice::Option(1) => (Approval::Approved, true),
        Choice::Option(_) => (Approval::Declined, false),
    };
    if switch_auto {
        model.approval_mode = ApprovalMode::Auto;
    }
    let (level, label) = match decision {
        Approval::Approved => (NoticeLevel::Info, format!("✓ {}", pending.action())),
        Approval::Declined => (
            NoticeLevel::Error,
            format!("✗ recusado: {}", pending.action()),
        ),
        Approval::Aborted => (NoticeLevel::Error, "✗ sessão encerrada".to_string()),
    };
    model.transcript.push(TranscriptItem::Notice(level, label));
    vec![Effect::AnswerApproval(decision)]
}

/// Drive the plan box shown after a plan-mode turn: arrows move the highlight, Enter/digits pick an
/// option, Esc cancels, Ctrl+C quits. "Executar" emits `ApprovePlan`; "Continuar" just closes the box
/// (staying in plan mode); "Cancelar" closes it and returns to default mode.
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
        Key::Esc => 2,
        _ => return vec![],
    };

    model.pending_plan = None;
    match index {
        // Execute the plan: the runtime leaves plan mode and runs a turn that carries it out.
        0 => vec![Effect::ApprovePlan],
        // Keep planning: close the box and stay in plan mode for more input.
        1 => vec![],
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

    #[test]
    fn typing_then_enter_submits_a_prompt() {
        let mut m = Model::default();
        for c in "hi".chars() {
            on_key(&mut m, press(Key::Char(c)));
        }
        let effects = on_key(&mut m, press(Key::Enter));
        assert_eq!(effects, vec![Effect::SubmitPrompt("hi".to_string())]);
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
    fn ctrl_c_cancels_a_running_turn_else_quits() {
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
        assert_eq!(on_key(&mut m, ctrl_c.clone()), vec![Effect::CancelTurn]);
        m.busy = false;
        assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
        assert!(m.should_quit);
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
        use crate::modules::agent::application::approval_policy::ApprovalMode;
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("p".to_string(), true)),
            ..Model::default()
        };
        on_key(&mut m, press(Key::Down)); // highlight option 2 ("…modo auto")
        assert_eq!(m.pending_approval.as_ref().unwrap().selected, 1);
        let effects = on_key(&mut m, press(Key::Enter));
        assert_eq!(effects, vec![Effect::AnswerApproval(Approval::Approved)]);
        assert_eq!(m.approval_mode, ApprovalMode::Auto);
        assert!(m.pending_approval.is_none());
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
    fn back_tab_cycles_the_approval_mode_when_idle() {
        use crate::modules::agent::application::approval_policy::ApprovalMode;
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
        use crate::modules::agent::application::approval_policy::ApprovalMode;
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
        use crate::modules::agent::application::approval_policy::ApprovalMode;
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
        let mut m = Model {
            pending_plan: Some(PendingPlan::default()),
            ..Model::default()
        };
        assert_eq!(on_key(&mut m, press(Key::Enter)), vec![Effect::ApprovePlan]);
        assert!(m.pending_plan.is_none());
    }

    #[test]
    fn plan_keep_planning_closes_box_and_stays_in_plan() {
        use crate::modules::agent::application::approval_policy::ApprovalMode;
        let mut m = Model {
            pending_plan: Some(PendingPlan::default()),
            approval_mode: ApprovalMode::Plan,
            ..Model::default()
        };
        on_key(&mut m, press(Key::Down)); // highlight "Continuar planejando"
        assert!(on_key(&mut m, press(Key::Enter)).is_empty());
        assert!(m.pending_plan.is_none());
        assert_eq!(m.approval_mode, ApprovalMode::Plan);
    }

    #[test]
    fn plan_cancel_leaves_plan_mode() {
        use crate::modules::agent::application::approval_policy::ApprovalMode;
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
    fn ctrl_c_mid_menu_aborts_to_quit_not_navigation() {
        let mut m = Model::default();
        type_str(&mut m, "/");
        let ctrl_c = KeyPress {
            code: Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
        assert!(m.should_quit);
    }
}
