//! Shared `#[cfg(test)]` helpers for the provider SSE adapters.

use crate::modules::provider::application::completion_provider::EventSink;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::stream_event::StreamEvent;

/// An [`EventSink`] that records every streamed delta, so an SSE test can assert the exact live event
/// sequence the accumulator emitted. Shared by both chat adapters' test modules.
#[derive(Default)]
pub(crate) struct CollectSink(pub Vec<StreamEvent>);

impl EventSink for CollectSink {
    fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError> {
        self.0.push(event);
        Ok(())
    }
}
