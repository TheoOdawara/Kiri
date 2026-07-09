use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Credential;

/// One [`Credential`] per provider id. Synchronous because the file backend is blocking and the calls
/// are rare. A missing credential is `Ok(None)` — the "not logged in yet" signal the wiring branches on.
pub trait SecretStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<Credential>, AgentError>;
    fn set(&self, provider_id: &str, credential: &Credential) -> Result<(), AgentError>;
    /// Deleting a missing key is a no-op, so callers may ignore the result best-effort.
    fn delete(&self, provider_id: &str) -> Result<(), AgentError>;
}
