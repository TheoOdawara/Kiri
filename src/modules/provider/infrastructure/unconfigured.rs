//! The null [`CompletionProvider`] injected when the harness boots with no usable credential, so the TUI
//! can still come up in onboarding instead of aborting.
//!
//! It is deliberately unreachable from [`super::factory::build_provider`]'s `(kind, auth)` match — only
//! `app::wire` constructs it — and the TUI's submit gate normally stops a turn before it arrives here.
//! This is the backstop behind that gate.

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
    use crate::modules::provider::application::completion_provider::NullSink;

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
