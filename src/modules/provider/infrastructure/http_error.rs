use crate::shared::kernel::error::AgentError;

/// An error body can echo the request, which may carry file contents the agent read, so the transcript
/// gets a preview rather than the whole thing.
const MAX_ERROR_BODY_CHARS: usize = 600;

pub(crate) fn bounded_preview(text: &str) -> String {
    if text.chars().count() <= MAX_ERROR_BODY_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX_ERROR_BODY_CHARS).collect();
    format!("{head}… (truncated)")
}

fn truncate_body(body: String) -> String {
    bounded_preview(&body)
}

/// A 4xx means the body we sent is unacceptable, so [`AgentError::ProviderRejected`] tells the frontend
/// to drop the turn rather than resend it forever. Everything else is transient.
pub(crate) fn error_from_status(status: reqwest::StatusCode, body: String) -> AgentError {
    // 429 and 408 are 4xx but transient throttles, and the same request succeeds after a backoff —
    // rejecting them would silently lose the user's message on free tiers.
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
    {
        return AgentError::Provider(format!(
            "provider throttled ({status}): {}",
            truncate_body(body)
        ));
    }
    // #58: redirects are disabled (credential must not be replayed to Location). Surface that clearly.
    if status.is_redirection() {
        return AgentError::Provider(format!(
            "the provider redirected this request (HTTP {status}); Kiri does not follow redirects \
             to avoid leaking your API key to an unverified host — point base_url at the final \
             endpoint directly. body: {}",
            truncate_body(body)
        ));
    }
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

    #[test]
    fn redirect_explains_that_kiri_does_not_follow() {
        let error = error_from_status(
            reqwest::StatusCode::TEMPORARY_REDIRECT,
            "go elsewhere".to_string(),
        );
        match error {
            AgentError::Provider(msg) => {
                assert!(msg.contains("does not follow redirects"), "{msg}");
                assert!(msg.contains("base_url"), "{msg}");
            }
            other => panic!("expected Provider, got {other:?}"),
        }
    }

    #[test]
    fn rate_limit_and_request_timeout_are_transient_not_rejected() {
        for status in [
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::REQUEST_TIMEOUT,
        ] {
            let error = error_from_status(status, "slow down".to_string());
            assert!(
                matches!(error, AgentError::Provider(_)),
                "{status} should be transient, got {error:?}"
            );
        }
    }
}
