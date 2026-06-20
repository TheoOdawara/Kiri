use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};

use crate::models::chat::{ChatRequest, ChatStreamChunk, Delta, StreamChoice, ToolCallFragment};
use crate::models::tools::{FunctionCall, ToolCall};

/// A semantic piece of the streamed completion: the model's reasoning, or its answer content.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Reasoning(String),
    Content(String),
}

/// The assembled result of one streamed assistant turn: any answer text, the tool calls it requested
/// (assembled from their streamed fragments, ordered by index), and the terminating finish reason.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedTurn {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

/// Send `request` to the OpenAI-compatible `<base_url>/chat/completions` endpoint and stream the
/// response: `on_event` fires for every reasoning or content delta as it arrives, and the assembled
/// turn (content + tool calls + finish reason) is returned once the stream ends.
pub async fn stream_completion(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ChatRequest,
    mut on_event: impl FnMut(StreamEvent) -> Result<()>,
) -> Result<CompletedTurn> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let mut response = client
        .post(&url)
        .bearer_auth(api_key)
        .json(request)
        .send()
        .await
        .context("failed to reach provider")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("provider returned {status}: {body}");
    }

    let mut accumulator = TurnAccumulator::default();
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.context("error reading stream")? {
        buffer.extend_from_slice(&chunk);
        while let Some(newline) = buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            handle_line(&line, &mut accumulator, &mut on_event)?;
        }
    }
    handle_line(&buffer, &mut accumulator, &mut on_event)?;

    Ok(accumulator.into_completed())
}

/// Parse one streamed line, feed both the accumulator (content/tool-calls/finish-reason) and the live
/// `on_event` callback (reasoning/content). Non-data lines, `[DONE]`, and malformed JSON are ignored.
fn handle_line(
    line: &[u8],
    accumulator: &mut TurnAccumulator,
    on_event: &mut impl FnMut(StreamEvent) -> Result<()>,
) -> Result<()> {
    let text = String::from_utf8_lossy(line);
    let Some(choice) = parse_chunk_line(text.trim_end()) else {
        return Ok(());
    };
    accumulator.absorb_tool_fragments(&choice.delta.tool_calls);
    if let Some(content) = &choice.delta.content {
        accumulator.content.push_str(content);
    }
    for event in events_from_delta(choice.delta) {
        on_event(event)?;
    }
    Ok(())
}

/// Parse one SSE line into its first choice. Yields nothing for non-`data:` lines, the `[DONE]`
/// sentinel, and malformed JSON.
fn parse_chunk_line(line: &str) -> Option<StreamChoice> {
    let data = line.strip_prefix("data:").map(str::trim)?;
    if data.is_empty() || data == "[DONE]" {
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
struct TurnAccumulator {
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

    fn into_completed(self) -> CompletedTurn {
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
                    arguments: partial.arguments,
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

    /// Parse one SSE line into events (test seam over `parse_chunk_line` + `events_from_delta`).
    fn events_from_sse_line(line: &str) -> Vec<StreamEvent> {
        match parse_chunk_line(line) {
            Some(choice) => events_from_delta(choice.delta),
            None => Vec::new(),
        }
    }

    /// Run lines through the same path as `stream_completion` and return the assembled turn.
    fn accumulate(lines: &[&str]) -> CompletedTurn {
        let mut accumulator = TurnAccumulator::default();
        {
            let mut sink = |_event: StreamEvent| -> Result<()> { Ok(()) };
            for line in lines {
                handle_line(line.as_bytes(), &mut accumulator, &mut sink).unwrap();
            }
        }
        accumulator.into_completed()
    }

    #[test]
    fn extracts_content_from_data_line() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hi"}}]}"#;
        assert_eq!(
            events_from_sse_line(line),
            vec![StreamEvent::Content("Hi".to_string())]
        );
    }

    #[test]
    fn extracts_reasoning_from_data_line() {
        let line = r#"data: {"choices":[{"delta":{"reasoning_content":"hmm"}}]}"#;
        assert_eq!(
            events_from_sse_line(line),
            vec![StreamEvent::Reasoning("hmm".to_string())]
        );
    }

    #[test]
    fn reasoning_field_is_accepted() {
        let line = r#"data: {"choices":[{"delta":{"reasoning":"hmm"}}]}"#;
        assert_eq!(
            events_from_sse_line(line),
            vec![StreamEvent::Reasoning("hmm".to_string())]
        );
    }

    #[test]
    fn duplicate_reasoning_keys_yield_single_event() {
        // NVIDIA Nemotron sends both keys in one delta; this must not fail to parse nor double up.
        let line =
            r#"data: {"choices":[{"delta":{"reasoning":"Okay","reasoning_content":"Okay"}}]}"#;
        assert_eq!(
            events_from_sse_line(line),
            vec![StreamEvent::Reasoning("Okay".to_string())]
        );
    }

    #[test]
    fn reasoning_and_content_in_one_delta_keep_order() {
        let line = r#"data: {"choices":[{"delta":{"reasoning":"why","content":"Hi"}}]}"#;
        assert_eq!(
            events_from_sse_line(line),
            vec![
                StreamEvent::Reasoning("why".to_string()),
                StreamEvent::Content("Hi".to_string()),
            ]
        );
    }

    #[test]
    fn non_string_reasoning_is_dropped_but_content_survives() {
        let line = r#"data: {"choices":[{"delta":{"reasoning":{"step":1},"content":"Hi"}}]}"#;
        assert_eq!(
            events_from_sse_line(line),
            vec![StreamEvent::Content("Hi".to_string())]
        );
    }

    #[test]
    fn done_sentinel_yields_nothing() {
        assert!(events_from_sse_line("data: [DONE]").is_empty());
    }

    #[test]
    fn non_data_or_blank_lines_yield_nothing() {
        assert!(events_from_sse_line("").is_empty());
        assert!(events_from_sse_line(": keep-alive").is_empty());
        assert!(events_from_sse_line("event: message").is_empty());
    }

    #[test]
    fn malformed_json_yields_nothing() {
        assert!(events_from_sse_line("data: {not json").is_empty());
    }

    #[test]
    fn role_only_or_empty_delta_yields_nothing() {
        let role_only = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert!(events_from_sse_line(role_only).is_empty());

        let empty = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        assert!(events_from_sse_line(empty).is_empty());
    }

    #[test]
    fn accumulates_single_tool_call_split_across_deltas() {
        let turn = accumulate(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"write_file","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.txt\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "data: [DONE]",
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
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","type":"function","function":{"name":"list_dir","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]);
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].function.name, "read_file");
        assert_eq!(turn.tool_calls[1].function.name, "list_dir");
    }

    #[test]
    fn tool_calls_accumulate_regardless_of_stop_finish_reason() {
        let turn = accumulate(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(turn.tool_calls.len(), 1);
    }

    #[test]
    fn defaults_function_type_when_provider_omits_it() {
        let turn = accumulate(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(turn.tool_calls[0].kind, "function");
    }

    #[test]
    fn content_and_reasoning_still_streamed_during_a_turn() {
        let mut accumulator = TurnAccumulator::default();
        let mut events = Vec::new();
        {
            let mut sink = |event: StreamEvent| -> Result<()> {
                events.push(event);
                Ok(())
            };
            handle_line(
                br#"data: {"choices":[{"delta":{"reasoning":"why","content":"Hi"}}]}"#,
                &mut accumulator,
                &mut sink,
            )
            .unwrap();
        }
        assert_eq!(
            events,
            vec![
                StreamEvent::Reasoning("why".to_string()),
                StreamEvent::Content("Hi".to_string()),
            ]
        );
        assert_eq!(accumulator.into_completed().content, "Hi");
    }
}
