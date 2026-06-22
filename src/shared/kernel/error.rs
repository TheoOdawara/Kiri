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
}
