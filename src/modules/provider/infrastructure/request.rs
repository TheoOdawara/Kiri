//! Small request-building idioms shared by the OpenAI-compatible adapters (and the URL join by
//! Anthropic): the base-URL join that tolerates a trailing slash, and the keyless-aware bearer header.
//! Sibling to `http_error` (which classifies the *response*); this shapes the *request*.

use crate::shared::kernel::provider::Secret;

/// Join an endpoint `suffix` onto a configured `base_url`, tolerating a trailing slash on the base so a
/// hand-edited `http://host/` and `http://host` both yield `http://host/{suffix}` rather than a `//`.
pub(crate) fn join_url(base: &str, suffix: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), suffix)
}

/// Attach `Authorization: Bearer <key>` only when a key is present. A keyless endpoint (local LM Studio
/// / Ollama) must send NO `Authorization` header — sending `Bearer ` (empty) makes some local servers
/// reject the request — so `None` leaves the builder untouched. Exposes the secret only at this one call
/// site.
pub(crate) fn apply_optional_bearer(
    request: reqwest::RequestBuilder,
    api_key: &Option<Secret>,
) -> reqwest::RequestBuilder {
    match api_key {
        Some(key) => request.bearer_auth(key.expose()),
        None => request,
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_optional_bearer, join_url};
    use crate::shared::kernel::provider::Secret;

    #[test]
    fn join_url_trims_trailing_slash() {
        assert_eq!(join_url("http://x/", "embeddings"), "http://x/embeddings");
        assert_eq!(join_url("http://x", "embeddings"), "http://x/embeddings");
        assert_eq!(
            join_url("http://x/v1/", "chat/completions"),
            "http://x/v1/chat/completions"
        );
    }

    #[test]
    fn apply_optional_bearer_omits_header_when_keyless() {
        // The locked LM Studio / Ollama regression: a keyless adapter sends no Authorization header.
        let client = reqwest::Client::new();
        let keyless = apply_optional_bearer(client.get("http://x/"), &None)
            .build()
            .unwrap();
        assert!(
            keyless
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .is_none()
        );

        let keyed = apply_optional_bearer(client.get("http://x/"), &Some(Secret::new("k")))
            .build()
            .unwrap();
        assert_eq!(
            keyed.headers().get(reqwest::header::AUTHORIZATION).unwrap(),
            "Bearer k"
        );
    }
}
