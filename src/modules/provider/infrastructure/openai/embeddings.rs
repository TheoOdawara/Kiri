use serde::{Deserialize, Serialize};

use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::modules::provider::infrastructure::http_error::error_from_status;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Secret;

/// OpenAI-compatible embeddings adapter (`POST {base_url}/embeddings`). NVIDIA and OpenAI expose this
/// endpoint; Anthropic does not, so the factory refuses to build one for an Anthropic profile. Reuses the
/// same timed `reqwest::Client` and `error_from_status` classification as the chat adapter.
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    /// Held as a `Secret` (zeroized on drop, redacted in Debug), exposed only at the auth-header site.
    api_key: Secret,
    model: String,
}

impl OpenAiEmbeddingProvider {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Secret,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key,
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
    /// The position of this vector's input in the request. The endpoint may return data out of order,
    /// so we sort by it rather than trusting arrival order (a misalignment silently corrupts recall).
    #[serde(default)]
    index: usize,
    embedding: Vec<f32>,
}

/// Reorder the response by `index` and verify there is exactly one vector per input, so a provider
/// returning rows out of order or with a different count can never silently misalign vectors with texts.
fn align_embeddings(
    mut data: Vec<EmbeddingDatum>,
    expected: usize,
) -> Result<Vec<Vec<f32>>, AgentError> {
    if data.len() != expected {
        return Err(AgentError::Provider(format!(
            "embeddings count mismatch: expected {expected}, got {}",
            data.len()
        )));
    }
    data.sort_by_key(|datum| datum.index);
    Ok(data.into_iter().map(|datum| datum.embedding).collect())
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
            .bearer_auth(self.api_key.expose())
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
        align_embeddings(parsed.data, texts.len())
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::{EmbeddingDatum, align_embeddings};

    #[test]
    fn align_reorders_by_index_and_checks_count() {
        let data = vec![
            EmbeddingDatum {
                index: 1,
                embedding: vec![2.0],
            },
            EmbeddingDatum {
                index: 0,
                embedding: vec![1.0],
            },
        ];
        assert_eq!(
            align_embeddings(data, 2).unwrap(),
            vec![vec![1.0], vec![2.0]]
        );
    }

    #[test]
    fn align_rejects_a_count_mismatch() {
        let data = vec![EmbeddingDatum {
            index: 0,
            embedding: vec![1.0],
        }];
        assert!(align_embeddings(data, 2).is_err());
    }
}
