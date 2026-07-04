//! The driven port for durable credential storage. Adapters live in
//! `provider/infrastructure/secrets` (a 0600 credentials file). Harness-owned storage — never
//! the agent's sandbox — so it sits alongside the `memory` context's carve-out from the
//! filesystem-sandbox invariant (ADR 0010).

use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::Credential;

/// Stores one [`Credential`] per provider id. Synchronous: the file backend is blocking and
/// calls are rare (composition root, `/provider` wizard, token refresh), so there is no value in an
/// async surface. A missing credential is `Ok(None)`, never an error — that is the "not logged in yet"
/// signal the wiring branches on.
pub trait SecretStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<Credential>, AgentError>;
    fn set(&self, provider_id: &str, credential: &Credential) -> Result<(), AgentError>;
    /// Remove a provider's stored credential. Live on the keyless path: boot (`app::wire`) and the
    /// runtime provider swap clear any stale key when an active or newly-added provider is keyless
    /// (`AuthMethod::None`), so a migrated keyed-to-keyless config leaves no orphaned secret behind. A
    /// missing-key delete is a harmless no-op, so both call sites can ignore the result best-effort.
    fn delete(&self, provider_id: &str) -> Result<(), AgentError>;
}
