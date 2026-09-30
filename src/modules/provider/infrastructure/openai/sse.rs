use std::collections::BTreeMap;

use super::arguments::normalize_arguments;
#[cfg(test)]
use super::wire::StreamChoice;
use super::wire::{ChatStreamChunk, Delta, StreamError, ToolCallFragment};
use crate::modules::provider::application::completion_provider::EventSink;
use crate::modules::provider::infrastructure::http_error::bounded_preview;
use crate::modules::provider::infrastructure::streaming::{
    enforce_stream_budget, is_empty_truncation,
};
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::stream_event::StreamEvent;
use crate::shared::kernel::tool_call::{FunctionCall, TOOL_CALL_FUNCTION_KIND, ToolCall};

/// The `[DONE]` sentinel and malformed JSON are ignored; line framing is handled upstream by
/// `eventsource-stream`.
pub(crate) fn handle_event(
    data: &str,
    accumulator: &mut TurnAccumulator,
    sink: &mut dyn EventSink,
) -> Result<(), AgentError> {
    if data.is_empty() || data == SSE_DONE_SENTINEL {
        return Ok(());
    }
    // A non-JSON line (keep-alive, unknown event) is ignored.
    let Ok(chunk) = serde_json::from_str::<ChatStreamChunk>(data) else {
        return Ok(());
    };
    // An in-band error on a 200 stream must fail the turn: swallowing it left an empty turn with no
    // feedback — a phantom "plan ready" box, the model never appearing to have been contacted.
    if let Some(error) = &chunk.error {
        return Err(AgentError::Provider(format_stream_error(error)));
    }
    let Some(choice) = chunk.choices.into_iter().next() else {
        return Ok(());
    };

    // Reasoning counts toward the same ceiling as content, or a provider streaming reasoning forever
    // would defeat the cap. Checked before absorbing, so an unbounded stream fails fast.
    let delta_bytes = choice.delta.content.as_deref().map_or(0, str::len)
        + choice
            .delta
            .reasoning_content
            .as_deref()
            .map_or(0, str::len)
        + choice.delta.reasoning.as_deref().map_or(0, str::len)
        + choice
            .delta
            .tool_calls
            .iter()
            .filter_map(|fragment| fragment.function.as_ref()?.arguments.as_deref())
            .map(str::len)
            .sum::<usize>();
    enforce_stream_budget(&mut accumulator.streamed_bytes, delta_bytes)?;

    if let Some(reason) = &choice.finish_reason {
        accumulator.finish_reason = Some(reason.clone());
    }
    accumulator.absorb_tool_fragments(&choice.delta.tool_calls);
    if let Some(content) = &choice.delta.content {
        accumulator.content.push_str(content);
    }
    for event in events_from_delta(choice.delta) {
        sink.on_event(event)?;
    }
    Ok(())
}

/// The provider's `message` and `code` are both untrusted text, so both are bounded before reaching the
/// transcript.
fn format_stream_error(error: &StreamError) -> String {
    let message = error
        .message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(bounded_preview)
        .unwrap_or_else(|| "unknown error".to_string());
    match error.code.as_ref().filter(|code| !code.is_null()) {
        Some(code) => {
            let code = bounded_preview(
                &code
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| code.to_string()),
            );
            format!("stream error from provider: {message} (code {code})")
        }
        None => format!("stream error from provider: {message}"),
    }
}

const SSE_DONE_SENTINEL: &str = "[DONE]";

/// What the API sends when the output token cap truncated the turn.
const FINISH_REASON_LENGTH: &str = "length";

/// A test seam; the live path uses `handle_event`.
#[cfg(test)]
fn parse_chunk(data: &str) -> Option<StreamChoice> {
    if data.is_empty() || data == SSE_DONE_SENTINEL {
        return None;
    }
    let chunk: ChatStreamChunk = serde_json::from_str(data).ok()?;
    chunk.choices.into_iter().next()
}

/// Reasoning precedes content; empty strings are dropped.
fn events_from_delta(delta: Delta) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    let reasoning = delta
        .reasoning_content
        .or(delta.reasoning)
        .filter(|text| !text.is_empty());
    if let Some(reasoning) = reasoning {
        events.push(StreamEvent::Reasoning(reasoning));
    }
    if let Some(content) = delta.content.filter(|text| !text.is_empty()) {
        events.push(StreamEvent::Content(content));
    }
    events
}

#[derive(Default)]
pub(crate) struct TurnAccumulator {
    content: String,
    /// A `BTreeMap` so the streamed `index` keys keep the tool calls in natural order.
    tool_calls: BTreeMap<u32, PartialToolCall>,
    streamed_bytes: usize,
    finish_reason: Option<String>,
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

impl TurnAccumulator {
    /// The provider surfaces this as an error, rather than return a turn that silently did nothing.
    pub(crate) fn hit_empty_output_limit(&self) -> bool {
        is_empty_truncation(
            self.finish_reason.as_deref() == Some(FINISH_REASON_LENGTH),
            &self.content,
            self.tool_calls.is_empty(),
        )
    }

    fn absorb_tool_fragments(&mut self, fragments: &[ToolCallFragment]) {
        for fragment in fragments {
            let slot = self.tool_calls.entry(fragment.index).or_default();
            if let Some(id) = &fragment.id {
                slot.id = id.clone();
            }
            if let Some(kind) = &fragment.kind {
                slot.kind = kind.clone();
            }
            if let Some(function) = &fragment.function {
                if let Some(name) = &function.name {
                    slot.name = name.clone();
                }
                if let Some(arguments) = &function.arguments {
                    slot.arguments.push_str(arguments);
                }
            }
        }
    }

    pub(crate) fn into_completed(self) -> CompletedTurn {
        let tool_calls = self
            .tool_calls
            .into_values()
            .map(|partial| ToolCall {
                id: partial.id,
                kind: if partial.kind.is_empty() {
                    TOOL_CALL_FUNCTION_KIND.to_string()
                } else {
                    partial.kind
                },
                function: FunctionCall {
                    name: partial.name,
                    arguments: normalize_arguments(partial.arguments),
                },
            })
            .collect();
        CompletedTurn {
            content: self.content,
            tool_calls,
            thinking: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::provider::infrastructure::test_support::CollectSink;

    fn events_from_data(data: &str) -> Vec<StreamEvent> {
        match parse_chunk(data) {
            Some(choice) => events_from_delta(choice.delta),
            None => Vec::new(),
        }
    }

    fn accumulate(payloads: &[&str]) -> CompletedTurn {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        for data in payloads {
            handle_event(data, &mut accumulator, &mut sink).unwrap();
        }
        accumulator.into_completed()
    }

    #[test]
    fn extracts_content_from_data_line() {
        let line = r#"{"choices":[{"delta":{"content":"Hi"}}]}"#;
        assert_eq!(
            events_from_data(line),
            vec![StreamEvent::Content("Hi".to_string())]
        );
    }

    #[test]
    fn extracts_reasoning_from_data_line() {
        let line = r#"{"choices":[{"delta":{"reasoning_content":"hmm"}}]}"#;
        assert_eq!(
            events_from_data(line),
            vec![StreamEvent::Reasoning("hmm".to_string())]
        );
    }

    #[test]
    fn reasoning_field_is_accepted() {
        let line = r#"{"choices":[{"delta":{"reasoning":"hmm"}}]}"#;
        assert_eq!(
            events_from_data(line),
            vec![StreamEvent::Reasoning("hmm".to_string())]
        );
    }

    #[test]
    fn duplicate_reasoning_keys_yield_single_event() {
        // NVIDIA Nemotron sends both keys in one delta.
        let line = r#"{"choices":[{"delta":{"reasoning":"Okay","reasoning_content":"Okay"}}]}"#;
        assert_eq!(
            events_from_data(line),
            vec![StreamEvent::Reasoning("Okay".to_string())]
        );
    }

    #[test]
    fn reasoning_and_content_in_one_delta_keep_order() {
        let line = r#"{"choices":[{"delta":{"reasoning":"why","content":"Hi"}}]}"#;
        assert_eq!(
            events_from_data(line),
            vec![
                StreamEvent::Reasoning("why".to_string()),
                StreamEvent::Content("Hi".to_string()),
            ]
        );
    }

    #[test]
    fn non_string_reasoning_is_dropped_but_content_survives() {
        let line = r#"{"choices":[{"delta":{"reasoning":{"step":1},"content":"Hi"}}]}"#;
        assert_eq!(
            events_from_data(line),
            vec![StreamEvent::Content("Hi".to_string())]
        );
    }

    #[test]
    fn done_sentinel_yields_nothing() {
        assert!(events_from_data("[DONE]").is_empty());
    }

    #[test]
    fn length_finish_with_no_output_is_flagged_as_truncated() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        handle_event(
            r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
            &mut accumulator,
            &mut sink,
        )
        .unwrap();
        assert!(accumulator.hit_empty_output_limit());
    }

    #[test]
    fn length_finish_with_partial_content_is_not_flagged_as_empty() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        handle_event(
            r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":"length"}]}"#,
            &mut accumulator,
            &mut sink,
        )
        .unwrap();
        assert!(!accumulator.hit_empty_output_limit());
    }

    #[test]
    fn in_band_error_event_is_surfaced_as_an_error() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        let data = r#"{"error":{"message":"Internal server error","type":"internal_server_error","code":500}}"#;
        let error = handle_event(data, &mut accumulator, &mut sink)
            .expect_err("an in-band error event must fail the turn");
        let message = error.to_string();
        assert!(
            message.contains("Internal server error"),
            "the provider message must be surfaced: {message}"
        );
        assert!(
            message.contains("500"),
            "the code must be surfaced: {message}"
        );
    }

    #[test]
    fn null_error_field_is_not_treated_as_an_error() {
        // Some providers include `"error": null` on success.
        let turn = accumulate(&[r#"{"error":null,"choices":[{"delta":{"content":"Hi"}}]}"#]);
        assert_eq!(turn.content, "Hi");
    }

    #[test]
    fn in_band_error_message_is_bounded() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        let data = serde_json::json!({ "error": { "message": "x".repeat(5_000), "code": 500 } })
            .to_string();
        let error = handle_event(&data, &mut accumulator, &mut sink)
            .expect_err("an in-band error event must fail the turn");
        assert!(
            error.to_string().contains("… (truncated)"),
            "oversized in-band error must be bounded: {error}"
        );
    }

    #[test]
    fn oversized_error_code_is_bounded() {
        // PROV-06: bounding only `message` is bypassed by moving the payload into the string-typed `code`.
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        let data = serde_json::json!({ "error": { "message": "x", "code": "C".repeat(5_000) } })
            .to_string();
        let error = handle_event(&data, &mut accumulator, &mut sink)
            .expect_err("an in-band error event must fail the turn");
        let surfaced = error.to_string();
        assert!(
            surfaced.contains("… (truncated)"),
            "oversized error code must be bounded: {surfaced}"
        );
        assert!(
            !surfaced.contains(&"C".repeat(5_000)),
            "the oversized error code must be truncated, not surfaced verbatim"
        );
    }

    #[test]
    fn reasoning_stream_exceeding_cap_fails() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        let chunk = serde_json::json!({
            "choices": [{ "delta": { "reasoning_content": "a".repeat(1024 * 1024) } }]
        })
        .to_string();
        let mut error = None;
        for _ in 0..16 {
            if let Err(err) = handle_event(&chunk, &mut accumulator, &mut sink) {
                error = Some(err);
                break;
            }
        }
        let error = error.expect("reasoning past MAX_STREAM_BYTES must fail");
        assert!(
            error.to_string().contains("maximum response size"),
            "reasoning cap error expected: {error}"
        );
    }

    #[test]
    fn non_data_or_blank_lines_yield_nothing() {
        assert!(events_from_data("").is_empty());
        assert!(events_from_data(": keep-alive").is_empty());
        assert!(events_from_data("event: message").is_empty());
    }

    #[test]
    fn malformed_json_yields_nothing() {
        assert!(events_from_data("{not json").is_empty());
    }

    #[test]
    fn role_only_or_empty_delta_yields_nothing() {
        let role_only = r#"{"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert!(events_from_data(role_only).is_empty());

        let empty = r#"{"choices":[{"delta":{"content":""}}]}"#;
        assert!(events_from_data(empty).is_empty());
    }

    #[test]
    fn accumulates_single_tool_call_split_across_deltas() {
        let turn = accumulate(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"write_file","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.txt\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        assert_eq!(turn.tool_calls.len(), 1);
        let call = &turn.tool_calls[0];
        assert_eq!(call.id, "call_1");
        assert_eq!(call.kind, "function");
        assert_eq!(call.function.name, "write_file");
        assert_eq!(call.function.arguments, r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn accumulates_parallel_tool_calls_by_index() {
        let turn = accumulate(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","type":"function","function":{"name":"list_dir","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]);
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].function.name, "read_file");
        assert_eq!(turn.tool_calls[1].function.name, "list_dir");
    }

    #[test]
    fn tool_calls_accumulate_regardless_of_stop_finish_reason() {
        let turn = accumulate(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(turn.tool_calls.len(), 1);
    }

    #[test]
    fn defaults_function_type_when_provider_omits_it() {
        let turn = accumulate(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(turn.tool_calls[0].kind, "function");
    }

    #[test]
    fn content_and_reasoning_still_streamed_during_a_turn() {
        let mut accumulator = TurnAccumulator::default();
        let mut sink = CollectSink::default();
        handle_event(
            r#"{"choices":[{"delta":{"reasoning":"why","content":"Hi"}}]}"#,
            &mut accumulator,
            &mut sink,
        )
        .unwrap();
        assert_eq!(
            sink.0,
            vec![
                StreamEvent::Reasoning("why".to_string()),
                StreamEvent::Content("Hi".to_string()),
            ]
        );
        assert_eq!(accumulator.into_completed().content, "Hi");
    }

    #[test]
    fn normalizes_raw_control_chars_in_tool_call_arguments() {
        // Built via `json!` so the chunk is valid wire JSON whose DECODED `arguments` carries the raw
        // 0x0A — the shape that poisons the conversation.
        let bad_args = "{\"path\":\"a.rs\",\"content\":\"line1\nline2\"}";
        let chunk = serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "c1",
                "type": "function",
                "function": {"name": "write_file", "arguments": bad_args}
            }]}}]
        })
        .to_string();

        let turn = accumulate(&[chunk.as_str()]);
        let args = &turn.tool_calls[0].function.arguments;
        let value: serde_json::Value =
            serde_json::from_str(args).expect("stored arguments must be valid JSON");
        assert_eq!(value["path"], "a.rs");
        assert_eq!(value["content"], "line1\nline2");
    }

    #[tokio::test]
    async fn eventsource_pipeline_frames_and_parses_a_raw_chunk() {
        use eventsource_stream::Eventsource;
        use tokio_stream::StreamExt;

        // Two SSE events in one raw byte blob, so the framing itself is exercised.
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n";
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
