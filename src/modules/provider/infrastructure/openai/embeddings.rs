use serde::{Deserialize, Serialize};

use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::modules::provider::infrastructure::request::{apply_optional_bearer, join_url};
use crate::modules::provider::infrastructure::streaming::{MAX_STREAM_BYTES, ensure_success};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Secret;

/// OpenAI-compatible embeddings adapter (`POST {base_url}/embeddings`). NVIDIA and OpenAI expose this
/// endpoint; Anthropic does not, so the factory refuses to build one for an Anthropic profile. Reuses the
/// same timed `reqwest::Client` and `error_from_status` classification as the chat adapter.
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    /// Optional API key (`Some` held as a `Secret`: zeroized on drop, redacted in Debug, exposed only at
    /// the auth-header site). `None` for a keyless local endpoint, in which case `embed` omits the header.
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
    // The count check alone passes a response with duplicate/missing indices (e.g. two rows at index 0),
    // which would return one input's vector for two texts. After sorting, the indices must be exactly
    // 0..expected for the position-to-input mapping to hold.
    if !data.iter().enumerate().all(|(i, datum)| datum.index == i) {
        return Err(AgentError::Provider(format!(
            "embeddings response has non-contiguous indices (expected 0..{expected}); a duplicate or missing index would misalign vectors with inputs"
        )));
    }
    Ok(data.into_iter().map(|datum| datum.embedding).collect())
}

/// Read the response body with a byte ceiling, mirroring the chat path's `MAX_STREAM_BYTES`, so a hostile
/// or misbehaving embeddings endpoint cannot exhaust memory through an unbounded JSON body. The advertised
/// `content-length` is rejected up front; the post-read recheck is the real guard for a chunked body that
/// advertises no length.
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
        // The count matches `expected`, so only the contiguity check can catch that input 1 has no
        // vector while input 0's vector is returned twice — the silent recall corruption PROV-04 guards.
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
        // Indices {0, 2}: the count matches `expected = 2`, but index 1 is missing, so the contiguity
        // check (not the count guard) is what must reject it.
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

    /// An embeddings endpoint advertising a body past `MAX_STREAM_BYTES` is rejected by the content-length
    /// pre-check before the body is read, so a hostile endpoint fails fast instead of exhausting memory.
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
                // Advertise a length past the cap; the pre-check fires before any body is consumed.
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

    /// A keyless embeddings endpoint (local LM Studio / Ollama) must omit the `Authorization` header,
    /// mirroring the chat adapter — `None` key sends no header at all.
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
