//! The driven port for durable credential storage. Adapters live in
//! `provider/infrastructure/secrets` (OS keyring; a 0600 file fallback). Harness-owned storage — never
//! the agent's sandbox — so it sits alongside the `memory` context's carve-out from the
//! filesystem-sandbox invariant (ADR 0010).

use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Credential;

/// Stores one [`Credential`] per provider id. Synchronous: backends (keyring, file) are blocking and
/// calls are rare (composition root, `/provider` wizard, token refresh), so there is no value in an
/// async surface. A missing credential is `Ok(None)`, never an error — that is the "not logged in yet"
/// signal the wiring branches on.
pub trait SecretStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<Credential>, AgentError>;
    fn set(&self, provider_id: &str, credential: &Credential) -> Result<(), AgentError>;
    /// Remove a provider's stored credential. Exercised by the file-store unit test and used by the
    /// `/provider` remove/logout flow (a later phase); kept here so the port models the full contract.
    #[allow(dead_code)]
    fn delete(&self, provider_id: &str) -> Result<(), AgentError>;
}
