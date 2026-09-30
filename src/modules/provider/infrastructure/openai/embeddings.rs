use serde::{Deserialize, Serialize};

use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::modules::provider::infrastructure::request::{apply_optional_bearer, join_url};
use crate::modules::provider::infrastructure::streaming::{MAX_STREAM_BYTES, ensure_success};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Secret;

/// `POST {base_url}/embeddings`. Anthropic exposes no such endpoint, so the factory refuses to build one
/// for an Anthropic profile.
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<Secret>,
    model: String,
}

impl OpenAiEmbeddingProvider {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Option<Secret>,
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
    /// The endpoint may return rows out of order, and a misalignment silently corrupts recall.
    #[serde(default)]
    index: usize,
    embedding: Vec<f32>,
}

/// Guarantees exactly one vector per input, in input order.
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
    // The count check alone passes two rows at index 0, which would hand one input's vector to two texts.
    if !data.iter().enumerate().all(|(i, datum)| datum.index == i) {
        return Err(AgentError::Provider(format!(
            "embeddings response has non-contiguous indices (expected 0..{expected}); a duplicate or missing index would misalign vectors with inputs"
        )));
    }
    Ok(data.into_iter().map(|datum| datum.embedding).collect())
}

/// The advertised `content-length` is rejected up front; the post-read recheck is the real guard for a
/// chunked body that advertises no length.
/// ponytail: a chunked body without content-length is still fully buffered by `bytes()` before the recheck;
/// upgrade path = a streamed, budget-bounded reader (like the chat SSE path) if a true cap is needed.
async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, AgentError> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_STREAM_BYTES as u64)
    {
        return Err(AgentError::Provider(format!(
            "embeddings response exceeds the maximum size ({MAX_STREAM_BYTES} bytes)"
        )));
    }
    let bytes = response.bytes().await.map_err(|error| {
        AgentError::Provider(format!("failed to read embeddings response: {error}"))
    })?;
    if bytes.len() > MAX_STREAM_BYTES {
        return Err(AgentError::Provider(format!(
            "embeddings response exceeds the maximum size ({MAX_STREAM_BYTES} bytes)"
        )));
    }
    Ok(bytes.to_vec())
}

#[async_trait::async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AgentError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = join_url(&self.base_url, "embeddings");
        let request = apply_optional_bearer(self.client.post(&url), &self.api_key);
        let response = request
            .json(&EmbeddingsRequest {
                model: &self.model,
                input: texts,
            })
            .send()
            .await
            .map_err(|error| {
                AgentError::Provider(format!("failed to reach embeddings endpoint: {error}"))
            })?;
        let response = ensure_success(response).await?;
        let bytes = bounded_body(response).await?;
        let parsed: EmbeddingsResponse = serde_json::from_slice(&bytes).map_err(|error| {
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
    use super::{EmbeddingDatum, OpenAiEmbeddingProvider, align_embeddings};
    use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
    use crate::modules::provider::infrastructure::test_support;
    use crate::shared::kernel::error::AgentError;

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

    #[test]
    fn align_rejects_duplicate_indices() {
        // The count matches, so only the contiguity check can reject this.
        let data = vec![
            EmbeddingDatum {
                index: 0,
                embedding: vec![1.0],
            },
            EmbeddingDatum {
                index: 0,
                embedding: vec![2.0],
            },
        ];
        assert!(align_embeddings(data, 2).is_err());
    }

    #[test]
    fn align_rejects_a_gap() {
        // The count matches, so only the contiguity check can reject this.
        let data = vec![
            EmbeddingDatum {
                index: 0,
                embedding: vec![1.0],
            },
            EmbeddingDatum {
                index: 2,
                embedding: vec![3.0],
            },
        ];
        assert!(align_embeddings(data, 2).is_err());
    }

    #[tokio::test]
    async fn oversized_embeddings_response_is_rejected() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                // A length past the cap, with no body: the pre-check must fire before any read.
                let huge = super::MAX_STREAM_BYTES + 1;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {huge}\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.flush().await;
            }
        });
        let base_url = format!("http://{addr}/v1");
        let provider =
            OpenAiEmbeddingProvider::new(reqwest::Client::new(), base_url, None, "embed-model");
        let result = provider.embed(&["hello".to_string()]).await;
        match result {
            Err(AgentError::Provider(message)) => assert!(
                message.contains("maximum size"),
                "expected a size-cap error, got: {message}"
            ),
            other => panic!("expected a provider size-cap error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn keyless_embeddings_omits_authorization_header() {
        let captured = test_support::capture_request(|base_url| async move {
            let provider =
                OpenAiEmbeddingProvider::new(reqwest::Client::new(), base_url, None, "embed-model");
            let _ = provider.embed(&["hello".to_string()]).await;
        })
        .await;
        assert!(
            !captured.to_ascii_lowercase().contains("authorization"),
            "keyless embeddings request must omit Authorization; got:\n{captured}"
        );
    }
}
