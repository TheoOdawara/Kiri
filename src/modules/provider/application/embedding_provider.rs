use crate::shared::kernel::error::AgentError;

/// A driven port to an embeddings endpoint, used by the memory context for semantic recall. Kept separate
/// from `CompletionProvider` because the active chat provider may be one (e.g. Anthropic) that exposes no
/// embeddings, while embeddings point at another. Unlike the streaming chat port this is `Send` (no
/// `EventSink`), so the `Send` memory port can await it.
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed each input text, returning one vector per input in the same order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AgentError>;

    /// The embedding model id, recorded alongside stored vectors so a later model change is detectable.
    fn model(&self) -> &str;
}
