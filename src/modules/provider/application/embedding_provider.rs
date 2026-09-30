use crate::shared::kernel::error::AgentError;

/// Separate from `CompletionProvider` because the active chat provider may expose no embeddings (e.g.
/// Anthropic) while embeddings point at another. Unlike the chat port this is `Send`, so the memory port
/// can await it.
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// One vector per input, in the same order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AgentError>;

    /// Recorded alongside stored vectors, so a later model change is detectable.
    fn model(&self) -> &str;
}
