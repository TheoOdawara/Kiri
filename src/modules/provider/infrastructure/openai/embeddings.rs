use serde::{Deserialize, Serialize};

use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::modules::provider::infrastructure::http_error::error_from_status;
use crate::shared::kernel::error::AgentError;

/// OpenAI-compatible embeddings adapter (`POST {base_url}/embeddings`). NVIDIA and OpenAI expose this
/// endpoint; Anthropic does not, so the factory refuses to build one for an Anthropic profile. Reuses the
/// same timed `reqwest::Client` and `error_from_status` classification as the chat adapter.
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiEmbeddingProvider {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

#[derive(Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
}

#[async_trait::async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AgentError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&EmbeddingsRequest {
                model: &self.model,
                input: texts,
            })
            .send()
            .await
            .map_err(|error| {
                AgentError::Provider(format!("failed to reach embeddings endpoint: {error}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<error body unavailable: {error}>"));
            return Err(error_from_status(status, body));
        }

        let parsed: EmbeddingsResponse = response.json().await.map_err(|error| {
            AgentError::Provider(format!("invalid embeddings response: {error}"))
        })?;
        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }

    fn model(&self) -> &str {
        &self.model
    }
}
