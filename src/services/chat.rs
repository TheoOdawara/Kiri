use anyhow::{Context, Result, bail};

use crate::models::chat::{ChatRequest, ChatStreamChunk, Delta};

/// A semantic piece of the streamed completion: the model's reasoning, or its answer content.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Reasoning(String),
    Content(String),
}

/// Send `request` to the OpenAI-compatible `<base_url>/chat/completions` endpoint and stream the
/// response, invoking `on_event` for every reasoning or content delta as it arrives.
pub async fn stream_completion(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ChatRequest,
    mut on_event: impl FnMut(StreamEvent) -> Result<()>,
) -> Result<()> {
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

    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.context("error reading stream")? {
        buffer.extend_from_slice(&chunk);
        while let Some(newline) = buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            emit_line(&line, &mut on_event)?;
        }
    }
    emit_line(&buffer, &mut on_event)?;

    Ok(())
}

fn emit_line(line: &[u8], on_event: &mut impl FnMut(StreamEvent) -> Result<()>) -> Result<()> {
    let text = String::from_utf8_lossy(line);
    for event in events_from_sse_line(text.trim_end()) {
        on_event(event)?;
    }
    Ok(())
}

/// Parse one SSE line into zero or more stream events. Yields nothing for non-`data:` lines, the
/// `[DONE]` sentinel, malformed JSON, and chunks without a textual delta.
fn events_from_sse_line(line: &str) -> Vec<StreamEvent> {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return Vec::new();
    };
    if data.is_empty() || data == "[DONE]" {
        return Vec::new();
    }
    let Ok(chunk) = serde_json::from_str::<ChatStreamChunk>(data) else {
        return Vec::new();
    };
    let Some(choice) = chunk.choices.into_iter().next() else {
        return Vec::new();
    };
    events_from_delta(choice.delta)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
