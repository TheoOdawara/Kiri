use crate::modules::agent::application::approval_policy::{Approval, ApprovalMode};
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Key, KeyPress};
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::domain::view_state::APPROVAL_OPTIONS;

const SCROLL_STEP: u16 = 5;

/// Interpret a key press against the current model, mutating it and returning any effects. Pure: no
/// I/O, so it is fully unit-testable. While an approval is pending, keys answer it; otherwise they
/// drive the editor, history, and scrollback.
pub fn on_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if model.pending_approval.is_some() {
        return on_approval_key(model, key);
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
            vec![]
        }
        Key::Enter => submit(model),
        Key::Char(c) if !key.ctrl => {
            model.input.insert_char(c);
            vec![]
        }
        Key::Backspace => {
            model.input.backspace();
            vec![]
        }
        Key::Delete => {
            model.input.delete();
            vec![]
        }
        Key::Left => {
            model.input.left();
            vec![]
        }
        Key::Right => {
            model.input.right();
            vec![]
        }
        Key::Home => {
            model.input.home();
            vec![]
        }
        Key::End => {
            model.input.end();
            vec![]
        }
        Key::Up => {
            if let Some(line) = model.history.older(model.input.text()) {
                model.input.set(line);
            }
            vec![]
        }
        Key::Down => {
            if let Some(line) = model.history.newer() {
                model.input.set(line);
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

/// Submit the editor contents: a quit command ends the session; anything else non-blank starts a turn.
fn submit(model: &mut Model) -> Vec<Effect> {
    if model.busy {
        return vec![];
    }
    let line = model.input.take();
    model.history.record(&line);
    model.scroll.pin();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::domain::view_state::PendingApproval;

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
}
