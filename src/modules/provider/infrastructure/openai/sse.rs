use std::collections::BTreeMap;

use super::arguments::normalize_arguments;
use super::wire::{ChatStreamChunk, Delta, StreamChoice, ToolCallFragment};
use crate::modules::agent::domain::completed_turn::CompletedTurn;
use crate::modules::agent::domain::stream_event::StreamEvent;
use crate::modules::provider::application::completion_provider::EventSink;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

/// Feed one parsed SSE event's `data` payload into the accumulator (content/tool-calls) and the live
/// `on_event` callback (reasoning/content). The `[DONE]` sentinel and malformed JSON are ignored. Line
/// framing (chunk reassembly, the `data:` prefix) is handled upstream by `eventsource-stream`.
pub(crate) fn handle_event(
    data: &str,
    accumulator: &mut TurnAccumulator,
    sink: &mut dyn EventSink,
) -> Result<(), AgentError> {
    // An OpenAI-compatible provider can deliver an error in-band on an HTTP 200 stream:
    // `data: {"error": {...}}` (then `[DONE]`). Surface it as a turn failure instead of silently
    // dropping the chunk — a swallowed in-band error left an empty turn with no feedback at all (a
    // phantom "plan ready" box in plan mode, with the model never appearing to have been contacted).
    if let Some(message) = parse_stream_error(data) {
        return Err(AgentError::Provider(message));
    }
    let Some(choice) = parse_chunk(data) else {
        return Ok(());
    };
    accumulator.absorb_tool_fragments(&choice.delta.tool_calls);
    if let Some(content) = &choice.delta.content {
        accumulator.content.push_str(content);
    }
    for event in events_from_delta(choice.delta) {
        sink.on_event(event)?;
    }
    Ok(())
}

/// Detect an in-band error payload (`{"error": {...}}`) the provider can stream on a 200 response,
/// returning a human-readable message. Yields nothing for a normal chunk, `[DONE]`, a non-JSON line,
/// or an `error` field that is explicitly `null` (some providers include a null `error` on success).
fn parse_stream_error(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    let error = value.get("error").filter(|error| !error.is_null())?;
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .unwrap_or("unknown error");
    match error.get("code").filter(|code| !code.is_null()) {
        Some(code) => {
            // Prefer a bare string code; otherwise its JSON form (e.g. the number 500).
            let code = code
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| code.to_string());
            Some(format!(
                "stream error from provider: {message} (code {code})"
            ))
        }
        None => Some(format!("stream error from provider: {message}")),
    }
}

/// The sentinel payload OpenAI-compatible SSE streams send to mark the end of a completion.
const SSE_DONE_SENTINEL: &str = "[DONE]";

/// Parse one event's `data` payload into its first choice. Yields nothing for the `[DONE]` sentinel,
/// an empty payload, or malformed JSON.
fn parse_chunk(data: &str) -> Option<StreamChoice> {
    if data.is_empty() || data == SSE_DONE_SENTINEL {
        return None;
    }
    let chunk: ChatStreamChunk = serde_json::from_str(data).ok()?;
    chunk.choices.into_iter().next()
}

/// Map a parsed delta to its events. Reasoning (under either field name) precedes content; empty
/// strings are dropped.
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

/// Assembles a turn from its streamed fragments. Tool calls are keyed by `index` (BTreeMap keeps them
/// in natural order); `function.arguments` slices are concatenated in arrival order.
#[derive(Default)]
pub(crate) struct TurnAccumulator {
    content: String,
    tool_calls: BTreeMap<u32, PartialToolCall>,
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

impl TurnAccumulator {
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
                    "function".to_string()
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse one event payload into events (test seam over `parse_chunk` + `events_from_delta`).
    fn events_from_data(data: &str) -> Vec<StreamEvent> {
        match parse_chunk(data) {
            Some(choice) => events_from_delta(choice.delta),
            None => Vec::new(),
        }
    }

    #[derive(Default)]
    struct CollectSink(Vec<StreamEvent>);

    impl EventSink for CollectSink {
        fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError> {
            self.0.push(event);
            Ok(())
        }
    }

    /// Run event payloads through the same path as the provider and return the assembled turn.
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
        // NVIDIA Nemotron sends both keys in one delta; this must not fail to parse nor double up.
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
    fn in_band_error_event_is_surfaced_as_an_error() {
        // Regression: NVIDIA can return HTTP 200 whose stream carries `{"error": {...}}` (a broken
        // model). This used to be dropped silently, leaving an empty turn; it must now fail the turn.
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
        // A normal chunk that carries `"error": null` (some providers include it on success) must not
        // abort the turn — its content still flows.
        let turn = accumulate(&[r#"{"error":null,"choices":[{"delta":{"content":"Hi"}}]}"#]);
        assert_eq!(turn.content, "Hi");
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
        // The model produced a file `content` with a RAW newline inside the string value instead of
        // `\n`. Built via `json!` so the SSE chunk is valid wire JSON whose decoded `arguments`
        // carries the literal 0x0A — exactly what poisons the conversation today. The stored value
        // must come out as valid JSON with the content preserved.
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

        // Two SSE events delivered as one raw byte blob: eventsource-stream frames them, we map the
        // first to a content event and the `[DONE]` sentinel to nothing.
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
