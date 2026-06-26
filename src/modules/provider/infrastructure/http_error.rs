//! Shared mapping of a non-success provider HTTP response into an [`AgentError`]. Both the OpenAI and
//! Anthropic adapters classify a failed request the same way — a 4xx means the body we sent is
//! unacceptable (resending it fails identically), anything else is a (typically transient) transport
//! failure — so the rule lives in one place rather than duplicated per adapter.

use crate::shared::kernel::error::AgentError;

/// Cap a provider error body before it reaches the transcript. The body can reflect the request we
/// sent (which may include file contents the agent read), so it is bounded to a short preview rather
/// than surfaced in full.
const MAX_ERROR_BODY_CHARS: usize = 600;

fn truncate_body(body: String) -> String {
    if body.chars().count() <= MAX_ERROR_BODY_CHARS {
        return body;
    }
    let head: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
    format!("{head}… (truncated)")
}

/// Classify a non-success response into the matching error. A 4xx becomes [`AgentError::ProviderRejected`]
/// (carrying the status + a bounded body) so the frontend can drop the offending turn instead of
/// resending it forever; a 5xx/other becomes a plain (transient) [`AgentError::Provider`].
pub(crate) fn error_from_status(status: reqwest::StatusCode, body: String) -> AgentError {
    if status.is_client_error() {
        AgentError::ProviderRejected {
            status: status.as_u16(),
            body: truncate_body(body),
        }
    } else {
        AgentError::Provider(format!(
            "provider returned {status}: {}",
            truncate_body(body)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ERROR_BODY_CHARS, error_from_status, truncate_body};
    use crate::shared::kernel::error::AgentError;

    #[test]
    fn truncate_body_keeps_short_bodies_verbatim() {
        let short = "invalid model".to_string();
        assert_eq!(truncate_body(short.clone()), short);
    }

    #[test]
    fn truncate_body_caps_long_bodies() {
        let out = truncate_body("x".repeat(5_000));
        assert!(out.ends_with("… (truncated)"));
        assert!(out.chars().count() <= MAX_ERROR_BODY_CHARS + 16);
    }

    #[test]
    fn client_error_becomes_provider_rejected_with_status_and_body() {
        let error = error_from_status(
            reqwest::StatusCode::BAD_REQUEST,
            "invalid model: nope".to_string(),
        );
        match error {
            AgentError::ProviderRejected { status, body } => {
                assert_eq!(status, 400);
                assert!(body.contains("invalid model"));
            }
            other => panic!("expected ProviderRejected, got {other:?}"),
        }
    }

    #[test]
    fn server_error_becomes_a_transient_provider_error() {
        let error = error_from_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "boom".to_string(),
        );
        assert!(matches!(error, AgentError::Provider(_)));
    }
}
