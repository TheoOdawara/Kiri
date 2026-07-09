use crate::shared::kernel::provider::Secret;

/// Tolerates a trailing slash on `base`, so a hand-edited `http://host/` does not yield a `//`.
pub(crate) fn join_url(base: &str, suffix: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), suffix)
}

/// A keyless endpoint (local LM Studio / Ollama) must send no `Authorization` header at all: some local
/// servers reject an empty `Bearer `.
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
