//! Assembles one streamed turn from the Messages API SSE events. Each event's `data` payload carries a
//! `type` discriminator (mirroring the SSE `event:` line), so dispatch is on the payload alone — the
//! `eventsource-stream` layer handles line framing upstream. Text deltas stream as content and
//! accumulate; thinking deltas stream as reasoning (not persisted); tool-use blocks accumulate their
//! id/name (from `content_block_start`) and JSON input (from `input_json_delta`), keyed by block index.

use std::collections::BTreeMap;

use super::wire::{BlockDeltaDto, ContentBlockStartDto, StreamEventDto};
use crate::modules::agent::domain::completed_turn::CompletedTurn;
use crate::modules::agent::domain::stream_event::StreamEvent;
use crate::modules::provider::application::completion_provider::EventSink;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

/// Feed one parsed SSE event's `data` payload into the accumulator and the live `sink`. A non-JSON
/// payload (keep-alive comment, blank line) is ignored; an `error` event fails the turn so a mid-stream
/// provider error is surfaced instead of silently truncating the turn.
pub(crate) fn handle_event(
    data: &str,
    accumulator: &mut TurnAccumulator,
    sink: &mut dyn EventSink,
) -> Result<(), AgentError> {
    let Ok(event) = serde_json::from_str::<StreamEventDto>(data) else {
        return Ok(());
    };
    match event {
        StreamEventDto::Error { error } => Err(AgentError::Provider(format!(
            "stream error from provider: {} ({})",
            error.message, error.kind
        ))),
        StreamEventDto::ContentBlockStart {
            index,
            content_block,
        } => {
            if let ContentBlockStartDto::ToolUse { id, name } = content_block {
                accumulator.start_tool_use(index, id, name);
            }
            Ok(())
        }
        StreamEventDto::ContentBlockDelta { index, delta } => {
            apply_delta(index, delta, accumulator, sink)
        }
        StreamEventDto::Other => Ok(()),
    }
}

fn apply_delta(
    index: u32,
    delta: BlockDeltaDto,
    accumulator: &mut TurnAccumulator,
    sink: &mut dyn EventSink,
) -> Result<(), AgentError> {
    match delta {
        BlockDeltaDto::TextDelta { text } if !text.is_empty() => {
            accumulator.content.push_str(&text);
            sink.on_event(StreamEvent::Content(text))
        }
        BlockDeltaDto::ThinkingDelta { thinking } if !thinking.is_empty() => {
            sink.on_event(StreamEvent::Reasoning(thinking))
        }
        BlockDeltaDto::InputJsonDelta { partial_json } => {
            accumulator.push_tool_input(index, &partial_json);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Assembles a turn from its streamed blocks. Tool-use blocks are keyed by their content-block `index`
/// (a `BTreeMap` keeps them in natural order); their `input_json_delta` slices concatenate in arrival
/// order. Text/thinking blocks need no slot — text accumulates into `content`, thinking only streams.
#[derive(Default)]
pub(crate) struct TurnAccumulator {
    content: String,
    tool_uses: BTreeMap<u32, PartialToolUse>,
}

#[derive(Default)]
struct PartialToolUse {
    id: String,
    name: String,
    input: String,
}

impl TurnAccumulator {
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

    /// Append a JSON-input slice to the tool-use block at `index`. An index with no started block is
    /// ignored — `input_json_delta` only follows a `tool_use` `content_block_start`.
    fn push_tool_input(&mut self, index: u32, partial: &str) {
        if let Some(slot) = self.tool_uses.get_mut(&index) {
            slot.input.push_str(partial);
        }
    }

    pub(crate) fn into_completed(self) -> CompletedTurn {
        let tool_calls = self
            .tool_uses
            .into_values()
            .map(|partial| ToolCall {
                id: partial.id,
                // The domain carries an OpenAI-style `type`; Anthropic tool_use blocks have none, so the
                // re-sent history uses the canonical "function".
                kind: "function".to_string(),
                function: FunctionCall {
                    name: partial.name,
                    arguments: tool_input_to_arguments(partial.input),
                },
            })
            .collect();
        CompletedTurn {
            content: self.content,
            tool_calls,
        }
    }
}

/// A tool call's assembled JSON input as the domain's `arguments` string. An empty input (a tool with
/// no parameters) becomes `"{}"`; a non-JSON input (a truncated stream) also falls back to `"{}"` so the
/// turn can never poison a later request.
fn tool_input_to_arguments(input: String) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "{}".to_string();
    }
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        input
    } else {
        "{}".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CollectSink(Vec<StreamEvent>);
    impl EventSink for CollectSink {
        fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError> {
            self.0.push(event);
            Ok(())
        }
    }

    /// Run a sequence of event payloads through the accumulator and return the collected live events
    /// plus the assembled turn.
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
    fn thinking_deltas_stream_as_reasoning_but_are_not_persisted() {
        let (events, turn) = run(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
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
    fn unknown_and_non_json_events_are_ignored() {
        let (events, turn) = run(&[
            "",
            ": keep-alive",
            r#"{"type":"message_start","message":{"id":"m","type":"message","role":"assistant"}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"x"}}"#,
        ]);
        assert!(events.is_empty());
        assert_eq!(turn.content, "");
        assert!(turn.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn eventsource_pipeline_frames_and_parses_a_raw_anthropic_stream() {
        use eventsource_stream::Eventsource;
        use tokio_stream::StreamExt;

        // A raw multi-event SSE blob (with both `event:` and `data:` lines, as Anthropic sends):
        // eventsource-stream frames it; we dispatch on the `data` payload's `type`.
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
