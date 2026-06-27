use crate::modules::session::domain::session::{Session, SessionSummary};
use crate::shared::kernel::error::AgentResult;
use crate::shared::kernel::message::Message;

/// Port for persisting conversations across sessions. Implemented by `SqliteSessionStore`
/// (`~/.kiri/sessions.db`). Sessions are keyed by `project_id` so a workspace lists only its own.
/// `init/create/append_messages/set_title/latest_for_project/list_for_project/load` are used by the
/// TUI runtime; `delete` prunes empty/aborted sessions.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Initialize storage (create the database and schema).
    async fn init(&self) -> AgentResult<()>;

    /// Create an empty session row for a project and return it.
    async fn create(&self, project_id: &str) -> AgentResult<Session>;

    /// Append messages to a session, advancing its `updated_at`. The caller passes only the new tail
    /// (the messages not yet persisted), in order.
    async fn append_messages(&self, session_id: &str, messages: &[Message]) -> AgentResult<()>;

    /// Set a session's title (derived from the first user message).
    async fn set_title(&self, session_id: &str, title: &str) -> AgentResult<()>;

    /// The most recently updated session for a project, if any — backs `/resume`.
    async fn latest_for_project(&self, project_id: &str) -> AgentResult<Option<SessionSummary>>;

    /// The most recent sessions for a project, newest first — backs the `/sessions` picker.
    async fn list_for_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<SessionSummary>>;

    /// Load a full session (all messages, in order) by id.
    async fn load(&self, session_id: &str) -> AgentResult<Option<Session>>;

    /// Delete a session and its messages. Returns whether a row was removed.
    #[allow(dead_code)]
    async fn delete(&self, session_id: &str) -> AgentResult<bool>;

    /// Whether the store initialized successfully; a degraded (inert) store reports `false`.
    fn is_available(&self) -> bool;
}
