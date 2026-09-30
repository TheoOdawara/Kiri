use crate::modules::session::domain::session::{Session, SessionSummary};
use crate::shared::kernel::error::AgentResult;
use crate::shared::kernel::message::Message;

#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn init(&self) -> AgentResult<()>;

    async fn create(&self, project_id: &str) -> AgentResult<Session>;

    /// `messages` is only the not-yet-persisted tail, in order — not the whole conversation.
    /// `base_index` (the tail's absolute position) and `salt` (the caller's per-conversation identity)
    /// derive a stable per-message identity, so a retried call with an unmoved cursor deduplicates
    /// silently — see `MESSAGE_UUID_NAMESPACE`.
    async fn append_messages(
        &self,
        session_id: &str,
        base_index: usize,
        salt: &str,
        messages: &[Message],
    ) -> AgentResult<()>;

    async fn set_title(&self, session_id: &str, title: &str) -> AgentResult<()>;

    async fn latest_for_project(&self, project_id: &str) -> AgentResult<Option<SessionSummary>>;

    async fn list_for_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<SessionSummary>>;

    async fn load(&self, session_id: &str) -> AgentResult<Option<Session>>;

    async fn recent_user_prompts(&self, project_id: &str, limit: usize)
    -> AgentResult<Vec<String>>;

    /// A degraded (inert) store reports `false`.
    fn is_available(&self) -> bool;
}
