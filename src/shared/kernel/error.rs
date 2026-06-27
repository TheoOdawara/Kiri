/// The error type returned across the harness's port boundaries. Adapters map their concrete failures
/// (HTTP, IO) into a variant; the binary edge converts it into `anyhow::Error` for free via `?`.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// A provider/transport failure (request failed, 5xx status, stream read error) — typically
    /// transient, so the conversation is kept and the request may be retried as-is.
    #[error("provider error: {0}")]
    Provider(String),
    /// The provider rejected the request body itself (HTTP 4xx): resending it unchanged fails
    /// identically, so the caller must repair or drop the offending turn before retrying.
    #[error("provider rejected the request (HTTP {status}): {body}")]
    ProviderRejected { status: u16, body: String },
    /// A terminal/IO failure while rendering or prompting.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A sandbox refusal: either a confinement-setup failure before spawning a tool's child process,
    /// or a path-resolution refusal at the filesystem chokepoint (traversal, a sensitive file name, a
    /// credential directory, a not-found target, or a path that escapes the workspace root).
    #[error("sandbox error: {0}")]
    Sandbox(String),
    /// A memory-store failure (file/SQLite persistence, serialization) in the `memory` context. The
    /// harness's own knowledge store is auxiliary, so callers degrade gracefully rather than abort.
    #[error("memory error: {0}")]
    Memory(String),
    /// A session-store failure (SQLite persistence, serialization) in the `session` context. Conversation
    /// persistence is auxiliary, so callers degrade gracefully (an inert store) rather than abort.
    #[error("session error: {0}")]
    Session(String),
    /// A credential-store failure (OS keyring or the 0600 fallback file): reading, writing, or
    /// (de)serializing a provider's secret material. Never carries the secret itself.
    #[error("credential store error: {0}")]
    Secret(String),
    /// A profile-sync failure in the `sync` context (a `git` invocation, or applying a pulled config).
    /// Distinct from `Provider` so it is not mistaken for a retryable transport failure.
    #[error("sync error: {0}")]
    Sync(String),
}
