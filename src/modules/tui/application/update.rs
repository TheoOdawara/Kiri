use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::keymap;
use crate::modules::tui::application::msg::{Msg, StreamKind};
use crate::modules::tui::domain::model::Model;

/// The pure reducer: apply one message to the model and return the effects the runtime must perform.
/// No I/O, no engine handles — fully unit-testable.
pub fn update(model: &mut Model, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(key) => keymap::on_key(model, key),
        Msg::Paste(text) => {
            if model.pending_approval.is_none() {
                model.input.insert(&text);
            }
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
        Msg::ApprovalRequested(pending) => {
            model.pending_approval = Some(pending);
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
    use crate::modules::tui::domain::transcript::TranscriptItem;

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
