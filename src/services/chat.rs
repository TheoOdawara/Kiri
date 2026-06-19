use anyhow::{Context, Result, bail};

use crate::models::chat::{ChatRequest, ChatStreamChunk};

/// Send `request` to the OpenAI-compatible `<base_url>/chat/completions` endpoint and stream the
/// response, invoking `on_token` for every content delta as it arrives.
pub async fn stream_completion(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ChatRequest,
    mut on_token: impl FnMut(&str) -> Result<()>,
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
            emit_line(&line, &mut on_token)?;
        }
    }
    emit_line(&buffer, &mut on_token)?;

    Ok(())
}

fn emit_line(line: &[u8], on_token: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    let text = String::from_utf8_lossy(line);
    if let Some(token) = delta_from_sse_line(text.trim_end()) {
        on_token(&token)?;
    }
    Ok(())
}

/// Extract the content delta from a single SSE line. Returns `None` for non-`data:` lines, the
/// `[DONE]` sentinel, malformed JSON, and chunks without textual content (e.g. the role-only opener).
fn delta_from_sse_line(line: &str) -> Option<String> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }

    let chunk: ChatStreamChunk = serde_json::from_str(data).ok()?;
    chunk
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.delta.content)
        .filter(|content| !content.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_content_from_data_line() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hi"}}]}"#;
        assert_eq!(delta_from_sse_line(line), Some("Hi".to_string()));
    }

    #[test]
    fn done_sentinel_yields_none() {
        assert_eq!(delta_from_sse_line("data: [DONE]"), None);
    }

    #[test]
    fn non_data_or_blank_lines_yield_none() {
        assert_eq!(delta_from_sse_line(""), None);
        assert_eq!(delta_from_sse_line(": keep-alive"), None);
        assert_eq!(delta_from_sse_line("event: message"), None);
    }

    #[test]
    fn malformed_json_yields_none() {
        assert_eq!(delta_from_sse_line("data: {not json"), None);
    }

    #[test]
    fn role_only_or_empty_delta_yields_none() {
        let role_only = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert_eq!(delta_from_sse_line(role_only), None);

        let empty = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        assert_eq!(delta_from_sse_line(empty), None);
    }
}
