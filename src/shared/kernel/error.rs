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
    /// A credential-store failure (the 0600 credentials file): reading, writing, or
    /// (de)serializing a provider's secret material. Never carries the secret itself.
    #[error("credential store error: {0}")]
    Secret(String),
    /// A profile-sync failure in the `sync` context (a `git` invocation, or applying a pulled config).
    /// Distinct from `Provider` so it is not mistaken for a retryable transport failure.
    #[error("sync error: {0}")]
    Sync(String),
    /// A config read/encode/write failure while persisting a live `/models`/`/effort`/`/provider`
    /// change to the global `~/.kiri/config.toml`. Distinct from `Io` because the TOML encode failure is
    /// not an `io::Error`, and distinct from `Sandbox`/`Secret`.
    #[error("config error: {0}")]
    Config(String),
}

/// The result of any fallible port/adapter signature: `Result<T, AgentError>` named once so the error
/// type is explicit at every signature without re-declaring a `Result` shadow per module.
pub type AgentResult<T> = Result<T, AgentError>;

impl AgentError {
    /// Build an [`AgentError::Memory`] from any `Display` source. The single constructor the memory
    /// adapters (SQLite + file) and the sync NDJSON (de)serializer map their non-IO failures through, so
    /// the "stringify into this variant" rule lives here instead of being re-hand-rolled per adapter. It
    /// also gives the shared SQLite harness one canonical `fn(String) -> AgentError` to parameterize on.
    pub fn memory(error: impl std::fmt::Display) -> Self {
        Self::Memory(error.to_string())
    }

    /// Build an [`AgentError::Session`] from any `Display` source. The single constructor the session
    /// store maps its non-IO failures through (mirrors [`AgentError::memory`]).
    pub fn session(error: impl std::fmt::Display) -> Self {
        Self::Session(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_constructor_builds_memory_variant() {
        let error = AgentError::memory("disk full");
        assert!(matches!(&error, AgentError::Memory(message) if message == "disk full"));
    }

    #[test]
    fn session_constructor_builds_session_variant() {
        let error = AgentError::session("locked");
        assert!(matches!(&error, AgentError::Session(message) if message == "locked"));
    }

    #[test]
    fn constructor_accepts_any_display_source() {
        // A bare `&str`.
        assert!(matches!(AgentError::memory("plain"), AgentError::Memory(_)));

        // A real `io::Error` (one of the `Display` sources the adapters pass).
        let from_io = AgentError::memory(std::io::Error::other("io boom"));
        assert!(matches!(&from_io, AgentError::Memory(message) if message.contains("io boom")));

        // A rusqlite-style error stand-in: any `Display` type the SQLite adapters thread through.
        #[derive(Debug)]
        struct SqliteLike;
        impl std::fmt::Display for SqliteLike {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("sqlite failure")
            }
        }
        let from_sqlite = AgentError::session(SqliteLike);
        assert!(
            matches!(&from_sqlite, AgentError::Session(message) if message == "sqlite failure")
        );
    }
}
