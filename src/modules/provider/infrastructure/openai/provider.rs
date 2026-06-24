use eventsource_stream::Eventsource;
use tokio_stream::StreamExt;

use super::message_dto::MessageDto;
use super::sse::{TurnAccumulator, handle_event};
use super::wire::{ChatRequest, ChatTemplateKwargs};
use crate::modules::agent::domain::completed_turn::CompletedTurn;
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::shared::kernel::error::AgentError;

/// Cap a provider error body before it reaches the transcript. The body can reflect the request we
/// sent (which may include file contents the agent read), so it is bounded to a short preview rather
/// than surfaced in full.
const MAX_ERROR_BODY_CHARS: usize = 600;

fn truncate_body(body: String) -> String {
    if body.chars().count() <= MAX_ERROR_BODY_CHARS {
        return body;
    }
    let head: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
    format!("{head}… (truncated)")
}

/// OpenAI-compatible chat provider (NVIDIA today). Holds the HTTP client and endpoint/credentials;
/// translates a domain `TurnRequest` into the wire `ChatRequest`, streams the response forwarding
/// deltas to `sink`, and assembles the turn.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl CompletionProvider for OpenAiProvider {
    async fn complete(
        &self,
        request: TurnRequest<'_>,
        sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        let body = ChatRequest {
            model: request.model,
            messages: request.messages.iter().map(MessageDto::from).collect(),
            stream: true,
            chat_template_kwargs: Some(ChatTemplateKwargs { thinking: true }),
            tools: request.tools,
        };

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| AgentError::Provider(format!("failed to reach provider: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // A 4xx means the body we sent is unacceptable; resending it unchanged fails identically,
            // so it is surfaced distinctly to let the frontend drop the offending turn. 5xx/other stay
            // a plain (transient) provider error.
            return Err(if status.is_client_error() {
                AgentError::ProviderRejected {
                    status: status.as_u16(),
                    body: truncate_body(body),
                }
            } else {
                AgentError::Provider(format!(
                    "provider returned {status}: {}",
                    truncate_body(body)
                ))
            });
        }

        let mut accumulator = TurnAccumulator::default();
        let stream = response.bytes_stream().eventsource();
        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            let event = event
                .map_err(|error| AgentError::Provider(format!("error reading stream: {error}")))?;
            handle_event(&event.data, &mut accumulator, sink)?;
        }

        Ok(accumulator.into_completed())
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ERROR_BODY_CHARS, truncate_body};

    #[test]
    fn truncate_body_keeps_short_bodies_verbatim() {
        let short = "invalid model".to_string();
        assert_eq!(truncate_body(short.clone()), short);
    }

    #[test]
    fn truncate_body_caps_long_bodies() {
        let out = truncate_body("x".repeat(5_000));
        assert!(out.ends_with("… (truncated)"));
        assert!(out.chars().count() <= MAX_ERROR_BODY_CHARS + 16);
    }
}
