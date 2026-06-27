//! The null-object [`CompletionProvider`]: a placeholder the composition root injects when the harness
//! boots with no usable credential (first run, no env key). It performs no network I/O and fails every
//! turn with a clear, actionable error, so the TUI can come up in onboarding and the user can configure a
//! provider via the wizard — which swaps in a real adapter through `AgentLoop::set_provider`.
//!
//! It is deliberately NOT reachable from [`super::factory::build_provider`]'s `(kind, auth)` match, so no
//! profile can ever select it; only `app::wire` constructs it. The submit gate in the TUI normally keeps
//! a turn from reaching it at all — this is the defense-in-depth backstop behind that gate.

use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::shared::kernel::completed_turn::CompletedTurn;
use crate::shared::kernel::error::AgentError;

#[derive(Debug, Default, Clone, Copy)]
pub struct UnconfiguredProvider;

impl UnconfiguredProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait(?Send)]
impl CompletionProvider for UnconfiguredProvider {
    async fn complete(
        &self,
        _request: TurnRequest<'_>,
        _sink: &mut dyn EventSink,
    ) -> Result<CompletedTurn, AgentError> {
        Err(AgentError::Provider(
            "no provider configured — use /provider to add an API key".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::stream_event::StreamEvent;

    /// An `EventSink` double that accepts every event — the null provider must error before it streams.
    struct NullSink;
    impl EventSink for NullSink {
        fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn complete_always_errors_with_a_provider_message() {
        let provider = UnconfiguredProvider::new();
        let mut sink = NullSink;
        let result = provider
            .complete(
                TurnRequest {
                    messages: &[],
                    model: "",
                    tools: &[],
                },
                &mut sink,
            )
            .await;
        match result {
            Err(AgentError::Provider(message)) => assert!(message.contains("/provider")),
            other => panic!("expected a Provider error, got {other:?}"),
        }
    }
}
