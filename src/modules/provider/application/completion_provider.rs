use serde_json::Value;

use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::stream_event::StreamEvent;

pub struct TurnRequest<'a> {
    pub messages: &'a [Message],
    pub model: &'a str,
    /// Opaque JSON the registry produced, passed to the provider verbatim.
    pub tools: &'a [Value],
}

/// A trait object rather than a generic closure, so `complete` stays dyn-compatible.
pub trait EventSink {
    fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError>;
}

/// Discards every delta, for headless completions that only need the assembled turn.
pub struct NullSink;

impl EventSink for NullSink {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
pub trait CompletionProvider: Send + Sync {
    /// Fires `sink` for each reasoning/content delta and returns the assembled turn.
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
