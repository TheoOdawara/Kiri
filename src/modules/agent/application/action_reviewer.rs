use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::tools::application::command_sandbox::SandboxGuarantees;
use crate::shared::kernel::error::AgentResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionReviewRequest {
    pub user_intent: String,
    pub tool: String,
    pub action: String,
    pub workspace: String,
    pub guarantees: SandboxGuarantees,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Allow,
    Deny,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionReview {
    pub decision: ReviewDecision,
    pub reason: String,
}

#[async_trait::async_trait(?Send)]
pub trait ActionReviewer: Send + Sync {
    async fn review(
        &self,
        provider: &dyn CompletionProvider,
        model: &str,
        request: &ActionReviewRequest,
    ) -> AgentResult<ActionReview>;
}

#[derive(Debug)]
pub struct RefuseActionReviewer;

#[async_trait::async_trait(?Send)]
impl ActionReviewer for RefuseActionReviewer {
    async fn review(
        &self,
        _provider: &dyn CompletionProvider,
        _model: &str,
        _request: &ActionReviewRequest,
    ) -> AgentResult<ActionReview> {
        Ok(ActionReview {
            decision: ReviewDecision::Uncertain,
            reason: "no autonomous action reviewer is configured".to_string(),
        })
    }
}
