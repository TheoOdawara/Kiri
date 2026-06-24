use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::keymap;
use crate::modules::tui::application::msg::{Msg, StreamKind};
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};

/// The pure reducer: apply one message to the model and return the effects the runtime must perform.
/// No I/O, no engine handles — fully unit-testable.
pub fn update(model: &mut Model, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(key) => keymap::on_key(model, key),
        Msg::Paste(text) => {
            if model.pending_approval.is_none() && model.pending_plan.is_none() {
                model.input.insert(&text);
                keymap::sync_menu(model);
            }
            Vec::new()
        }
        Msg::ImageAttached(attachment) => {
            let label = format!(
                "🖼 imagem anexada ({}×{})",
                attachment.width, attachment.height
            );
            model.attachments.push(attachment);
            model
                .transcript
                .push(TranscriptItem::Notice(NoticeLevel::Info, label));
            Vec::new()
        }
        Msg::Resize => Vec::new(),
        Msg::Tick => {
            if model.status.streaming {
                model.status.spinner_frame = model.status.spinner_frame.wrapping_add(1);
            }
            Vec::new()
        }
        Msg::TurnBegan => {
            model.status.streaming = true;
            Vec::new()
        }
        Msg::StreamDelta(StreamKind::Reasoning, text) => {
            model.transcript.push_reasoning_delta(&text);
            Vec::new()
        }
        Msg::StreamDelta(StreamKind::Content, text) => {
            model.transcript.push_content_delta(&text);
            Vec::new()
        }
        Msg::TurnFinished => {
            model.status.streaming = false;
            Vec::new()
        }
        Msg::ToolStarted { command, diff } => {
            model.transcript.push_tool_start(command, diff);
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
            model.scroll.up(3);
            Vec::new()
        }
        Msg::ScrollDown => {
            model.scroll.down(3);
            Vec::new()
        }
        Msg::TurnEnded => {
            model.busy = false;
            model.status.streaming = false;
            model.pending_approval = None;
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
}
