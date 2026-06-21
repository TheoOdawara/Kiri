use anyhow::{Context, Result, bail};

use super::sse::{TurnAccumulator, handle_line};
use super::wire::ChatRequest;
use crate::modules::agent::domain::completed_turn::CompletedTurn;
use crate::modules::agent::domain::stream_event::StreamEvent;

/// Send `request` to the OpenAI-compatible `<base_url>/chat/completions` endpoint and stream the
/// response: `on_event` fires for every reasoning or content delta as it arrives, and the assembled
/// turn (content + tool calls) is returned once the stream ends.
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
