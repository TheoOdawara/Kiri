use std::time::Duration;

use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::keymap;
use crate::modules::tui::application::msg::{Msg, StreamKind};
use crate::modules::tui::domain::model::Model;

/// Backdating the open instant by more than the splash settle window instantly freezes the breath-in.
const SPLASH_FAST_FORWARD: Duration = Duration::from_millis(700);

/// The pure reducer: apply one message to the model and return the effects the runtime must perform.
/// No I/O, no engine handles — fully unit-testable.
pub fn update(model: &mut Model, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(key) => {
            // Any keypress fast-forwards the splash breath-in so a user who opens Kiri dozens of times a
            // day never waits on chrome. Backdating the open instant settles it on the next frame.
            if let Some(now) = model.timeline.render_at {
                model.timeline.opened_at =
                    Some(now.checked_sub(SPLASH_FAST_FORWARD).unwrap_or(now));
            }
            keymap::on_key(model, key)
        }
        Msg::Mouse { kind, col, row } => keymap::on_mouse(model, kind, col, row),
        Msg::Paste(text) => {
            if let Some(wizard) = model.wizard.as_mut() {
                // Route a paste into the wizard's active field — the common way an API key is entered.
                // It must NEVER fall through to the plaintext composer, where a pasted key would be
                // unmasked, survive the modal, and could be sent to the provider as a prompt.
                wizard.push_str(&text);
            } else if model.pending_approval.is_none()
                && model.pending_plan.is_none()
                && model.picker.is_none()
            {
                model.input.insert(&text);
                keymap::sync_menu(model);
            }
            Vec::new()
        }
        Msg::ImageAttached(attachment) => {
            let label = format!(
                "🖼 imagem anexada ({}×{}) — vai junto no próximo envio",
                attachment.width, attachment.height
            );
            model.attachments.push(attachment);
            model.notify_info(label);
            Vec::new()
        }
        // A reflow makes the (col,row) selection anchors meaningless — drop it so the overlay never
        // paints unrelated cells.
        Msg::Resize => {
            model.clear_screen_selection();
            Vec::new()
        }
        // The spinner frame is derived from wall-clock elapsed in the runtime's draw step, so it
        // animates at a steady rate regardless of message cadence. A tick just keeps the loop
        // redrawing on schedule.
        Msg::Tick => Vec::new(),
        Msg::TurnBegan => {
            model.status.streaming = true;
            model.status.turn_failed = false;
            model.timeline.begin_turn();
            Vec::new()
        }
        Msg::StreamDelta(StreamKind::Reasoning, text) => {
            model.transcript.push_reasoning_delta(&text);
            Vec::new()
        }
        Msg::StreamDelta(StreamKind::Content, text) => {
            // Keep the line-landing buffer aligned to the current answer: a content delta that starts a
            // fresh assistant item (e.g. after a tool ran) resets the cooling state. Each completed line
            // (a `\n`) lands at the current frame's instant, driving its cooling-steel reveal.
            if !model.transcript.last_is_assistant() {
                model.timeline.reset_stream();
            }
            if let Some(now) = model.timeline.render_at {
                for _ in 0..text.matches('\n').count() {
                    model.timeline.push_stream_landing(now);
                }
            }
            model.transcript.push_content_delta(&text);
            Vec::new()
        }
        Msg::TurnFinished => {
            model.status.streaming = false;
            Vec::new()
        }
        Msg::ToolStarted {
            command,
            diff,
            is_run_command,
        } => {
            model
                .transcript
                .push_tool_start(command, diff, is_run_command);
            Vec::new()
        }
        Msg::ToolFinished {
            status,
            output,
            elapsed,
        } => {
            model.transcript.finish_last_tool(status, output, elapsed);
            Vec::new()
        }
        Msg::ApprovalRequested(pending) => {
            model.pending_approval = Some(pending);
            Vec::new()
        }
        Msg::ScrollUp => {
            model.clear_screen_selection();
            model.scroll.up(keymap::WHEEL_STEP);
            Vec::new()
        }
        Msg::ScrollDown => {
            model.clear_screen_selection();
            model.scroll.down(keymap::WHEEL_STEP);
            Vec::new()
        }
        Msg::TurnEnded => {
            model.busy = false;
            model.status.streaming = false;
            model.pending_approval = None;
            model.timeline.settle_turn();
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::domain::transcript::{ToolStatus, TranscriptItem};
    use std::time::Duration;

    #[test]
    fn stream_deltas_build_transcript_items() {
        let mut m = Model::default();
        update(&mut m, Msg::StreamDelta(StreamKind::Content, "Hel".into()));
        update(&mut m, Msg::StreamDelta(StreamKind::Content, "lo".into()));
        assert_eq!(
            m.transcript.items(),
            &[TranscriptItem::Assistant("Hello".to_string())]
        );
    }

    #[test]
    fn tool_started_then_finished_build_one_tool_item() {
        let mut m = Model::default();
        update(
            &mut m,
            Msg::ToolStarted {
                command: "cat a.txt".into(),
                diff: None,
                is_run_command: false,
            },
        );
        match m.transcript.items() {
            [TranscriptItem::Tool(a)] => assert!(a.result.is_none(), "should be running"),
            other => panic!("expected one running tool item, got {other:?}"),
        }
        update(
            &mut m,
            Msg::ToolFinished {
                status: ToolStatus::Ok,
                output: "hello".into(),
                elapsed: Duration::from_millis(3),
            },
        );
        match m.transcript.items() {
            [TranscriptItem::Tool(a)] => {
                let (status, output, _) = a.result.as_ref().expect("finished");
                assert_eq!(*status, ToolStatus::Ok);
                assert_eq!(output, "hello");
            }
            other => panic!("expected one finished tool item, got {other:?}"),
        }
    }

    #[test]
    fn turn_ended_resets_per_turn_state() {
        let mut m = Model {
            busy: true,
            ..Model::default()
        };
        m.status.streaming = true;
        update(&mut m, Msg::TurnEnded);
        assert!(!m.busy);
        assert!(!m.status.streaming);
    }

    // --- screen selection invalidation --------------------------------------------

    use crate::modules::tui::domain::selection::{Granularity, ScreenSelection};

    /// A non-empty three-cell character selection.
    fn a_selection() -> ScreenSelection {
        let mut s = ScreenSelection::new(1, 1, Granularity::Char);
        s.extend(3, 1);
        s
    }

    #[test]
    fn scroll_clears_the_screen_selection() {
        let mut m = Model::default();
        m.selection.active = Some(a_selection());
        update(&mut m, Msg::ScrollUp);
        assert!(m.selection.active.is_none());
        m.selection.active = Some(a_selection());
        update(&mut m, Msg::ScrollDown);
        assert!(m.selection.active.is_none());
    }

    #[test]
    fn wheel_step_named_and_used() {
        // A wheel notch moves the named WHEEL_STEP lines, not a bare literal.
        let mut m = Model::default();
        update(&mut m, Msg::ScrollUp);
        assert_eq!(m.scroll.scrollback, keymap::WHEEL_STEP);
        update(&mut m, Msg::ScrollDown);
        assert_eq!(m.scroll.scrollback, 0);
    }

    #[test]
    fn paste_into_an_open_wizard_fills_the_field_not_the_composer() {
        // Regression: a pasted API key (the common way keys are entered) must land in the wizard's
        // masked, Secret-staged field — never in the plaintext composer, where it would be unmasked,
        // survive the modal, and could be sent to the provider as a prompt.
        use crate::modules::tui::domain::wizard::{ProviderWizard, WizardStep};
        let mut wizard = ProviderWizard::new();
        wizard.step = WizardStep::ApiKey;
        let mut m = Model {
            wizard: Some(wizard),
            ..Model::default()
        };
        update(&mut m, Msg::Paste("sk-ant-pasted\n".to_string()));
        assert_eq!(
            m.wizard.as_ref().unwrap().api_key,
            "sk-ant-pasted",
            "the key fills the wizard field (control chars filtered)"
        );
        assert!(m.input.is_empty(), "the plaintext composer must stay empty");
    }

    #[test]
    fn resize_clears_the_screen_selection() {
        let mut m = Model::default();
        m.selection.active = Some(a_selection());
        update(&mut m, Msg::Resize);
        assert!(m.selection.active.is_none());
    }

    #[test]
    fn streamed_content_keeps_the_screen_selection() {
        // Async content must NOT drop the selection: a delta arriving mid-drag/mid-copy would otherwise
        // make it impossible to select streaming output.
        let mut m = Model::default();
        m.selection.active = Some(a_selection());
        update(&mut m, Msg::StreamDelta(StreamKind::Content, "hi".into()));
        assert!(
            m.selection.active.is_some(),
            "a stream delta must keep the selection"
        );
        update(&mut m, Msg::Tick);
        assert!(
            m.selection.active.is_some(),
            "a tick must keep the selection"
        );
    }
}
