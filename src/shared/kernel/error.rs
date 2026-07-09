/// The error type returned across the harness's port boundaries. The binary edge converts it into
/// `anyhow::Error` for free via `?`.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Transport failure — typically transient, so the conversation is kept and the request may be
    /// retried as-is.
    #[error("provider error: {0}")]
    Provider(String),
    /// HTTP 4xx: resending the body unchanged fails identically, so the caller must repair or drop the
    /// offending turn before retrying.
    #[error("provider rejected the request (HTTP {status}): {body}")]
    ProviderRejected { status: u16, body: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A confinement-setup failure, or a path refused at the filesystem chokepoint.
    #[error("sandbox error: {0}")]
    Sandbox(String),
    /// The knowledge store is auxiliary, so callers degrade gracefully rather than abort.
    #[error("memory error: {0}")]
    Memory(String),
    /// Conversation persistence is auxiliary, so callers degrade to an inert store rather than abort.
    #[error("session error: {0}")]
    Session(String),
    /// A failure reading or writing the 0600 credentials file. Never carries the secret itself.
    #[error("credential store error: {0}")]
    Secret(String),
    /// Distinct from `Provider` so a `git` failure is not mistaken for a retryable transport one.
    #[error("sync error: {0}")]
    Sync(String),
    /// Distinct from `Io` because a TOML encode failure is not an `io::Error`.
    #[error("config error: {0}")]
    Config(String),
    /// ADR 0021. Extensions are auxiliary, so callers degrade gracefully rather than abort.
    #[error("extensions error: {0}")]
    Extensions(String),
}

/// Named once so the error type is explicit at every signature without a per-module `Result` shadow.
pub type AgentResult<T> = Result<T, AgentError>;

impl AgentError {
    /// The single stringify-into-this-variant point, so the rule is not re-hand-rolled per adapter. Also
    /// gives the shared SQLite harness one canonical `fn(String) -> AgentError` to parameterize on.
    pub fn memory(error: impl std::fmt::Display) -> Self {
        Self::Memory(error.to_string())
    }

    pub fn session(error: impl std::fmt::Display) -> Self {
        Self::Session(error.to_string())
    }

    pub fn extensions(error: impl std::fmt::Display) -> Self {
        Self::Extensions(error.to_string())
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
    fn extensions_constructor_builds_extensions_variant() {
        let error = AgentError::extensions("bad frontmatter");
        assert!(matches!(&error, AgentError::Extensions(message) if message == "bad frontmatter"));
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
