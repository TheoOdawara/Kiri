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

#[async_trait::async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AgentError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let mut request = self.client.post(&url);
        // Omit Authorization for a keyless endpoint (see the chat adapter); `Bearer ` empty is rejected
        // by some local servers.
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose());
        }
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
    use super::{EmbeddingDatum, OpenAiEmbeddingProvider, align_embeddings};
    use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
    use std::time::Duration;

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

    /// A keyless embeddings endpoint (local LM Studio / Ollama) must omit the `Authorization` header,
    /// mirroring the chat adapter — `None` key sends no header at all.
    #[tokio::test]
    async fn keyless_embeddings_omits_authorization_header() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let captured = String::from_utf8_lossy(&buf[..n]).into_owned();
                let body = "stop";
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
                let _ = tx.send(captured);
            }
        });

        let provider = OpenAiEmbeddingProvider::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1"),
            None,
            "embed-model",
        );
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            provider.embed(&["hello".to_string()]),
        )
        .await;
        let captured = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("server should capture the request")
            .expect("capture channel should deliver");
        assert!(
            !captured.to_ascii_lowercase().contains("authorization"),
            "keyless embeddings request must omit Authorization; got:\n{captured}"
        );
    }
}
