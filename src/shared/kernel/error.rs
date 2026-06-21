/// The error type returned across the harness's port boundaries. Adapters map their concrete failures
/// (HTTP, IO) into a variant; the binary edge converts it into `anyhow::Error` for free via `?`.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// A provider/transport failure (request failed, non-success status, stream read error).
    #[error("provider error: {0}")]
    Provider(String),
    /// A terminal/IO failure while rendering or prompting.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
