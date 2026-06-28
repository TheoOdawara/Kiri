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

/// An [`EventSink`] that discards every streamed delta. For headless completions (the distiller, which
/// only needs the assembled turn) and for tests that assert the final turn, not the live event sequence.
pub struct NullSink;

impl EventSink for NullSink {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_discards_and_returns_ok() {
        let mut sink = NullSink;
        assert!(sink.on_event(StreamEvent::Content("x".to_string())).is_ok());
    }
}
