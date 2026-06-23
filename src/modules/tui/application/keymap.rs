use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Key, KeyPress};
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};

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
        None if line.trim().is_empty() => vec![],
        None => {
            model.transcript.push(TranscriptItem::User(line.clone()));
            model.busy = true;
            vec![Effect::SubmitPrompt(line)]
        }
    }
}

/// Map a key to an approval decision, recording the outcome in the transcript.
fn on_approval_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    let default_accept = model
        .pending_approval
        .as_ref()
        .is_some_and(|p| p.default_accept);

    let approval = match key.code {
        Key::Esc => Some(Approval::Aborted),
        Key::Char('c') if key.ctrl => Some(Approval::Aborted),
        Key::Enter => Some(if default_accept {
            Approval::Approved
        } else {
            Approval::Declined
        }),
        Key::Char(c) => match c.to_ascii_lowercase() {
            's' | 'y' => Some(Approval::Approved),
            'n' => Some(Approval::Declined),
            _ => None,
        },
        _ => None,
    };

    match approval {
        Some(decision) => {
            let pending = model
                .pending_approval
                .take()
                .expect("an approval is pending");
            let (level, label) = match decision {
                Approval::Approved => (NoticeLevel::Info, format!("✓ {}", pending.prompt)),
                Approval::Declined => (
                    NoticeLevel::Error,
                    format!("✗ recusado: {}", pending.prompt),
                ),
                Approval::Aborted => (NoticeLevel::Error, "✗ sessão encerrada".to_string()),
            };
            model.transcript.push(TranscriptItem::Notice(level, label));
            vec![Effect::AnswerApproval(decision)]
        }
        None => vec![],
    }
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
            pending_approval: Some(PendingApproval {
                prompt: "delete a.txt".to_string(),
                default_accept: true,
            }),
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
            pending_approval: Some(PendingApproval {
                prompt: "p".to_string(),
                default_accept: false,
            }),
            ..Model::default()
        };
        assert_eq!(
            on_key(&mut m, press(Key::Enter)),
            vec![Effect::AnswerApproval(Approval::Declined)]
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
}
