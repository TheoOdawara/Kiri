use serde_json::Value;

use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::stream_event::StreamEvent;

/// What a provider needs to stream one assistant turn: the conversation so far (domain messages), the
/// model id, and the tool schemas to advertise (opaque JSON the registry produced, passed verbatim).
pub struct TurnRequest<'a> {
    pub messages: &'a [Message],
    pub model: &'a str,
    pub tools: &'a [Value],
}

/// Live sink for streamed deltas. A trait object (not a generic closure) so `complete` stays
/// dyn-compatible; the terminal UI implements it to render reasoning/content as it arrives.
pub trait EventSink {
    fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError>;
}

/// The agent loop's driven port to a chat provider. One adapter per provider (OpenAI-compatible
/// today); runtime-swappable behind `Arc<dyn CompletionProvider>`.
#[async_trait::async_trait(?Send)]
pub trait CompletionProvider: Send + Sync {
    /// Stream one assistant turn, firing `sink` for each reasoning/content delta, and return the
    /// assembled turn (answer text + any tool calls).
    async fn complete(
        &self,
        request: TurnRequest<'_>,
        sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError>;
}
