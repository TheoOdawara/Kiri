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
            // so it is surfaced distinctly to let the REPL drop the offending turn. 5xx/other stay a
            // plain (transient) provider error.
            return Err(if status.is_client_error() {
                AgentError::ProviderRejected {
                    status: status.as_u16(),
                    body,
                }
            } else {
                AgentError::Provider(format!("provider returned {status}: {body}"))
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
