//! Each event's `data` payload carries a `type` discriminator mirroring the SSE `event:` line, so
//! dispatch happens on the payload alone and the `event:` line is never read.

use std::collections::BTreeMap;

use super::wire::{BlockDelta, ContentBlockStart, MessageDelta, WireStreamEvent};
use crate::modules::provider::application::completion_provider::EventSink;
use crate::modules::provider::infrastructure::http_error::bounded_preview;
use crate::modules::provider::infrastructure::streaming::{
    enforce_stream_budget, is_empty_truncation,
};
use crate::modules::provider::infrastructure::tool_args;
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::ThinkingBlock;
use crate::shared::kernel::stream_event::StreamEvent;
use crate::shared::kernel::tool_call::{FunctionCall, TOOL_CALL_FUNCTION_KIND, ToolCall};

/// The Messages API `stop_reason` that means the output token cap truncated the turn.
const STOP_REASON_MAX_TOKENS: &str = "max_tokens";

/// A non-JSON payload (keep-alive, blank line) is ignored; an `error` event fails the turn rather than
/// truncate it silently.
pub(crate) fn handle_event(
    data: &str,
    accumulator: &mut TurnAccumulator,
    sink: &mut dyn EventSink,
) -> Result<(), AgentError> {
    let Ok(event) = serde_json::from_str::<WireStreamEvent>(data) else {
        return Ok(());
    };
    match event {
        WireStreamEvent::Error { error } => Err(AgentError::Provider(format!(
            "stream error from provider: {} ({})",
            bounded_preview(&error.message),
            bounded_preview(&error.kind)
        ))),
        WireStreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            match content_block {
                ContentBlockStart::ToolUse { id, name } => {
                    accumulator.start_tool_use(index, id, name)
                }
                // The encrypted blob has no readable summary, so it must never reach the UI sink.
                ContentBlockStart::RedactedThinking { data } => {
                    accumulator.set_redacted_thinking(index, data)
                }
                ContentBlockStart::Other => {}
            }
            Ok(())
        }
        WireStreamEvent::ContentBlockDelta { index, delta } => {
            apply_delta(index, delta, accumulator, sink)
        }
        WireStreamEvent::MessageDelta {
            delta: MessageDelta { stop_reason },
        } => {
            if let Some(reason) = stop_reason {
                accumulator.stop_reason = Some(reason);
            }
            Ok(())
        }
        WireStreamEvent::Other => Ok(()),
    }
}

fn apply_delta(
    index: u32,
    delta: BlockDelta,
    accumulator: &mut TurnAccumulator,
    sink: &mut dyn EventSink,
) -> Result<(), AgentError> {
    match delta {
        BlockDelta::TextDelta { text } if !text.is_empty() => {
            enforce_stream_budget(&mut accumulator.streamed_bytes, text.len())?;
            accumulator.content.push_str(&text);
            sink.on_event(StreamEvent::Content(text))
        }
        BlockDelta::ThinkingDelta { thinking } if !thinking.is_empty() => {
            // Thinking counts toward the same ceiling as text, or a provider streaming it forever OOMs.
            enforce_stream_budget(&mut accumulator.streamed_bytes, thinking.len())?;
            accumulator.push_thinking_text(index, &thinking);
            sink.on_event(StreamEvent::Reasoning(thinking))
        }
        BlockDelta::SignatureDelta { signature } if !signature.is_empty() => {
            // Never shown to the user, but it still counts toward the stream budget.
            enforce_stream_budget(&mut accumulator.streamed_bytes, signature.len())?;
            accumulator.push_thinking_signature(index, &signature);
            Ok(())
        }
        BlockDelta::InputJsonDelta { partial_json } => {
            enforce_stream_budget(&mut accumulator.streamed_bytes, partial_json.len())?;
            accumulator.push_tool_input(index, &partial_json);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A `BTreeMap` keeps the streamed content-block indices in natural order.
#[derive(Default)]
pub(crate) struct TurnAccumulator {
    content: String,
    tool_uses: BTreeMap<u32, PartialToolUse>,
    thinking: BTreeMap<u32, PartialThinking>,
    streamed_bytes: usize,
    stop_reason: Option<String>,
}

#[derive(Default)]
struct PartialToolUse {
    id: String,
    name: String,
    input: String,
}

/// `Visible` accumulates deltas; `Redacted` arrives whole and never changes.
enum PartialThinking {
    Visible { text: String, signature: String },
    Redacted { data: String },
}

impl PartialThinking {
    fn empty_visible() -> Self {
        Self::Visible {
            text: String::new(),
            signature: String::new(),
        }
    }
}

impl TurnAccumulator {
    /// The provider surfaces this as an error, rather than return a turn that silently did nothing.
    pub(crate) fn hit_empty_output_limit(&self) -> bool {
        is_empty_truncation(
            self.stop_reason.as_deref() == Some(STOP_REASON_MAX_TOKENS),
            &self.content,
            self.tool_uses.is_empty(),
        )
    }

    fn start_tool_use(&mut self, index: u32, id: String, name: String) {
        self.tool_uses.insert(
            index,
            PartialToolUse {
                id,
                name,
                input: String::new(),
            },
        );
    }

    /// An index with no started block is ignored: `input_json_delta` only follows a `tool_use` start.
    fn push_tool_input(&mut self, index: u32, partial: &str) {
        if let Some(slot) = self.tool_uses.get_mut(&index) {
            slot.input.push_str(partial);
        }
    }

    /// A delta for an index already recorded as `Redacted` is a defensive no-op: the protocol never
    /// streams one, but it must not panic or overwrite if it ever did.
    fn push_thinking_text(&mut self, index: u32, text: &str) {
        match self
            .thinking
            .entry(index)
            .or_insert_with(PartialThinking::empty_visible)
        {
            PartialThinking::Visible { text: existing, .. } => existing.push_str(text),
            PartialThinking::Redacted { .. } => {}
        }
    }

    /// Same defensive no-op as `push_thinking_text` for an index already recorded as `Redacted`.
    fn push_thinking_signature(&mut self, index: u32, signature: &str) {
        match self
            .thinking
            .entry(index)
            .or_insert_with(PartialThinking::empty_visible)
        {
            PartialThinking::Visible {
                signature: existing,
                ..
            } => existing.push_str(signature),
            PartialThinking::Redacted { .. } => {}
        }
    }

    /// The encrypted `data` arrives whole in `content_block_start`; there is no delta to accumulate.
    fn set_redacted_thinking(&mut self, index: u32, data: String) {
        self.thinking
            .insert(index, PartialThinking::Redacted { data });
    }

    pub(crate) fn into_completed(self) -> CompletedTurn {
        let tool_calls = self
            .tool_uses
            .into_values()
            .map(|partial| ToolCall {
                id: partial.id,
                // Anthropic `tool_use` blocks carry no `type`, so re-sent history uses the canonical kind.
                kind: TOOL_CALL_FUNCTION_KIND.to_string(),
                function: FunctionCall {
                    name: partial.name,
                    arguments: tool_args::sanitized_string(&partial.input),
                },
            })
            .collect();
        // Anthropic sends at most one thinking block per turn; the lowest index is the one to carry
        // forward should a future response ever stream more.
        let thinking = self
            .thinking
            .into_values()
            .next()
            .map(|partial| match partial {
                PartialThinking::Visible { text, signature } => ThinkingBlock::Visible {
                    text,
                    signature: (!signature.is_empty()).then_some(signature),
                },
                PartialThinking::Redacted { data } => ThinkingBlock::Redacted { data },
            });
        CompletedTurn {
            content: self.content,
            tool_calls,
            thinking,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::provider::infrastructure::test_support::CollectSink;

    fn run(payloads: &[&str]) -> (Vec<StreamEvent>, CompletedTurn) {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        for data in payloads {
            handle_event(data, &mut accumulator, &mut sink).unwrap();
        }
        (sink.0, accumulator.into_completed())
    }

    #[test]
    fn text_deltas_stream_and_accumulate() {
        let (events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        assert_eq!(
            events,
            vec![
                StreamEvent::Content("Hel".to_string()),
                StreamEvent::Content("lo".to_string()),
            ]
        );
        assert_eq!(turn.content, "Hello");
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn thinking_deltas_stream_as_reasoning_and_are_persisted() {
        let (events, turn) = run(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}"#,
        ]);
        assert_eq!(
            events,
            vec![
                StreamEvent::Reasoning("hmm".to_string()),
                StreamEvent::Content("Hi".to_string()),
            ]
        );
        assert_eq!(turn.content, "Hi");
        match turn.thinking.expect("the thinking block must be persisted") {
            ThinkingBlock::Visible { text, signature } => {
                assert_eq!(text, "hmm");
                assert_eq!(signature.as_deref(), Some("sig-1"));
            }
            other => panic!("expected Visible, got {other:?}"),
        }
    }

    #[test]
    fn thinking_text_and_signature_accumulate_across_multiple_deltas() {
        let (_events, turn) = run(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first "}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"second"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"ab"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"cd"}}"#,
        ]);
        match turn.thinking.expect("thinking must be present") {
            ThinkingBlock::Visible { text, signature } => {
                assert_eq!(text, "first second");
                assert_eq!(signature.as_deref(), Some("abcd"));
            }
            other => panic!("expected Visible, got {other:?}"),
        }
    }

    #[test]
    fn no_thinking_delta_leaves_thinking_absent() {
        let (_events, turn) = run(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
        ]);
        assert!(turn.thinking.is_none());
    }

    #[test]
    fn redacted_thinking_start_is_captured_without_streaming_to_the_sink() {
        let (events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"encrypted-blob"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}"#,
        ]);
        assert_eq!(events, vec![StreamEvent::Content("Hi".to_string())]);
        match turn.thinking.expect("the redacted block must be persisted") {
            ThinkingBlock::Redacted { data } => assert_eq!(data, "encrypted-blob"),
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn redacted_thinking_ignores_a_stray_delta_for_the_same_index() {
        // The protocol never streams this; the guard exists so it cannot panic or corrupt if it did.
        let (_events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"encrypted-blob"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"stray"}}"#,
        ]);
        match turn.thinking.expect("the redacted block must survive") {
            ThinkingBlock::Redacted { data } => assert_eq!(data, "encrypted-blob"),
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn tool_use_block_assembles_into_a_tool_call() {
        let (_events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"write_file","input":{}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ]);
        assert_eq!(turn.tool_calls.len(), 1);
        let call = &turn.tool_calls[0];
        assert_eq!(call.id, "toolu_1");
        assert_eq!(call.kind, "function");
        assert_eq!(call.function.name, "write_file");
        assert_eq!(call.function.arguments, r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn parallel_tool_uses_keep_their_indices() {
        let (_events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"read_file","input":{}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"b","name":"list_dir","input":{}}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
        ]);
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].function.name, "read_file");
        assert_eq!(turn.tool_calls[1].function.name, "list_dir");
    }

    #[test]
    fn tool_use_with_no_input_defaults_to_empty_object() {
        let (_events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"noop","input":{}}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ]);
        assert_eq!(turn.tool_calls[0].function.arguments, "{}");
    }

    #[test]
    fn truncated_tool_input_falls_back_to_empty_object() {
        let (_events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"x","input":{}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"p\":"}}"#,
        ]);
        assert_eq!(turn.tool_calls[0].function.arguments, "{}");
    }

    #[test]
    fn max_tokens_stop_with_no_output_is_flagged_as_truncated() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        handle_event(
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#,
            &mut accumulator,
            &mut sink,
        )
        .unwrap();
        assert!(accumulator.hit_empty_output_limit());
    }

    #[test]
    fn normal_stop_is_not_flagged_as_truncated() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        for data in [
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ] {
            handle_event(data, &mut accumulator, &mut sink).unwrap();
        }
        assert!(!accumulator.hit_empty_output_limit());
    }

    #[test]
    fn error_event_fails_the_turn() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        let error = handle_event(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            &mut accumulator,
            &mut sink,
        )
        .expect_err("an error event must fail the turn");
        let message = error.to_string();
        assert!(message.contains("Overloaded"), "message lost: {message}");
        assert!(message.contains("overloaded_error"), "kind lost: {message}");
    }

    #[test]
    fn in_band_error_message_is_bounded() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        let data = serde_json::json!({
            "type": "error",
            "error": { "type": "overloaded_error", "message": "x".repeat(5_000) }
        })
        .to_string();
        let error = handle_event(&data, &mut accumulator, &mut sink)
            .expect_err("an error event must fail the turn");
        assert!(
            error.to_string().contains("… (truncated)"),
            "oversized in-band error must be bounded: {error}"
        );
    }

    #[test]
    fn oversized_error_type_is_bounded() {
        // PROV-06: bounding only `message` is bypassed by moving the payload into `type`.
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        let data = serde_json::json!({
            "type": "error",
            "error": { "type": "T".repeat(5_000), "message": "x" }
        })
        .to_string();
        let error = handle_event(&data, &mut accumulator, &mut sink)
            .expect_err("an error event must fail the turn");
        let surfaced = error.to_string();
        assert!(
            surfaced.contains("… (truncated)"),
            "oversized error type must be bounded: {surfaced}"
        );
        assert!(
            !surfaced.contains(&"T".repeat(5_000)),
            "the oversized error type must be truncated, not surfaced verbatim"
        );
    }

    /// Crosses `MAX_STREAM_BYTES` with a handful of large chunks, never allocating the whole cap.
    fn accumulate_until_error(payload: &str) -> AgentError {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        for _ in 0..16 {
            if let Err(error) = handle_event(payload, &mut accumulator, &mut sink) {
                return error;
            }
        }
        panic!("a stream past MAX_STREAM_BYTES must fail");
    }

    #[test]
    fn anthropic_stream_exceeding_the_cap_fails_fast() {
        let chunk = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "a".repeat(1024 * 1024) }
        })
        .to_string();
        let error = accumulate_until_error(&chunk);
        assert!(
            error.to_string().contains("maximum response size"),
            "text-delta cap error expected: {error}"
        );
    }

    #[test]
    fn anthropic_reasoning_stream_exceeding_cap_fails() {
        let chunk = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "a".repeat(1024 * 1024) }
        })
        .to_string();
        let error = accumulate_until_error(&chunk);
        assert!(
            error.to_string().contains("maximum response size"),
            "reasoning cap error expected: {error}"
        );
    }

    #[test]
    fn anthropic_stream_exceeding_cap_fails_on_tool_input() {
        // The block must be started first, or its input slot would not exist.
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        handle_event(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"write_file","input":{}}}"#,
            &mut accumulator,
            &mut sink,
        )
        .unwrap();
        let chunk = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "input_json_delta", "partial_json": "a".repeat(1024 * 1024) }
        })
        .to_string();
        let mut error = None;
        for _ in 0..16 {
            if let Err(err) = handle_event(&chunk, &mut accumulator, &mut sink) {
                error = Some(err);
                break;
            }
        }
        let error = error.expect("tool-input past MAX_STREAM_BYTES must fail");
        assert!(
            error.to_string().contains("maximum response size"),
            "tool-input cap error expected: {error}"
        );
    }

    #[test]
    fn anthropic_normal_turn_under_cap_ok() {
        let (events, turn) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        assert_eq!(events, vec![StreamEvent::Content("Hello".to_string())]);
        assert_eq!(turn.content, "Hello");
    }

    #[test]
    fn unknown_and_non_json_events_are_ignored() {
        // A genuinely unrecognized kind, since `signature_delta` is now handled.
        let (events, turn) = run(&[
            "",
            ": keep-alive",
            r#"{"type":"message_start","message":{"id":"m","type":"message","role":"assistant"}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":"x"}}"#,
        ]);
        assert!(events.is_empty());
        assert_eq!(turn.content, "");
        assert!(turn.tool_calls.is_empty());
        assert!(turn.thinking.is_none());
    }

    #[tokio::test]
    async fn eventsource_pipeline_frames_and_parses_a_raw_anthropic_stream() {
        use eventsource_stream::Eventsource;
        use tokio_stream::StreamExt;

        // Both `event:` and `data:` lines, as Anthropic sends them; only the payload's `type` is read.
        let raw = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let stream = tokio_stream::iter(vec![Ok::<_, std::convert::Infallible>(
            raw.as_bytes().to_vec(),
        )]);
        let mut events = stream.eventsource();

        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        while let Some(event) = events.next().await {
            handle_event(&event.unwrap().data, &mut accumulator, &mut sink).unwrap();
        }
        assert_eq!(sink.0, vec![StreamEvent::Content("Hi".to_string())]);
        assert_eq!(accumulator.into_completed().content, "Hi");
    }
}
