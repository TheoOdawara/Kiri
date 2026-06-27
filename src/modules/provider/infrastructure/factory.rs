//! Selects and constructs the concrete provider adapter from a [`ProviderProfile`] and its
//! [`Credential`]. Shared by the composition root (`app::wire`, initial construction) and the TUI
//! runtime (a live `/provider`/`/effort` swap), so the `(kind, auth)` → adapter decision lives in one
//! place. Returns [`AgentError`] (not `anyhow`) since both callers are inside the engine boundary.

use std::sync::Arc;

use super::anthropic::provider::AnthropicProvider;
use super::openai::embeddings::OpenAiEmbeddingProvider;
use super::openai::provider::OpenAiProvider;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{
    AuthMethod, Credential, Effort, ProviderKind, ProviderProfile, Secret,
};

/// Construct the provider adapter for `profile` + `credential`. Two adapters cover every supported
/// provider: the Anthropic Messages API adapter for `Anthropic`, and the OpenAI-compatible
/// chat-completions adapter for NVIDIA, generic compatible endpoints, custom endpoints, and OpenAI
/// proper. The OpenAI-compatible adapter also serves keyless local endpoints (Ollama / LM Studio): an
/// `auth = "none"` profile builds with no key and the adapter omits the `Authorization` header. Vendor
/// kinds always require a key, so a keyless vendor profile fails fast. Subscription OAuth (Claude
/// Pro/Max, ChatGPT Plus/Pro) is intentionally unsupported — the vendors restrict those tokens to their
/// own clients, so it would require impersonation that risks the user's account (see the provider-auth
/// ADR); an `Oauth` profile fails fast with that rationale.
pub fn build_provider(
    client: reqwest::Client,
    profile: &ProviderProfile,
    credential: Credential,
    thinking: bool,
    effort: Effort,
) -> Result<Arc<dyn CompletionProvider>, AgentError> {
    // A blank model would otherwise surface as an opaque provider 400 on the first turn; fail fast.
    if profile.model.trim().is_empty() {
        return Err(AgentError::Provider(format!(
            "provider '{}' has no model configured; set its `model` in ~/.kiri/config.toml (NVIDIA users can export NVIDIA_MODEL for the default provider)",
            profile.id
        )));
    }
    match (profile.kind, &profile.auth) {
        // An auth value this build does not recognize leaves the provider inert with an actionable error,
        // rather than silently falling through to the OpenAI adapter via the catch-all below.
        (_, AuthMethod::Unknown(method)) => Err(AgentError::Provider(format!(
            "provider '{}' has an unrecognized auth method '{method}'; update Kiri or fix `auth` in ~/.kiri/config.toml",
            profile.id
        ))),
        (ProviderKind::Anthropic, AuthMethod::Oauth) => Err(AgentError::Provider(format!(
            "provider '{}' uses Anthropic subscription OAuth, which Kiri does not support — Anthropic restricts Pro/Max OAuth tokens to its own clients. Configure an Anthropic Console API key instead.",
            profile.id
        ))),
        (ProviderKind::Openai, AuthMethod::Oauth) => Err(AgentError::Provider(format!(
            "provider '{}' uses ChatGPT subscription OAuth, which Kiri does not support — a ChatGPT subscription does not include API access. Configure a platform.openai.com API key instead.",
            profile.id
        ))),
        // Vendor endpoints have no anonymous mode (NVIDIA/OpenAI need a Bearer key, Anthropic an
        // x-api-key); a keyless vendor profile (hand-edited or synced) fails fast instead of issuing
        // unauthenticated requests that 401 with a worse message. `requires_api_key` is the single
        // source of truth shared with the wizard, so the two cannot drift.
        (kind, AuthMethod::None) if kind.requires_api_key() => Err(AgentError::Provider(format!(
            "provider '{}' requires an API key but is configured with auth = \"none\"; only generic OpenAI-compatible / custom endpoints (e.g. Ollama, LM Studio) may be keyless",
            profile.id
        ))),
        (ProviderKind::Anthropic, AuthMethod::ApiKey) => {
            let key = api_key_of(credential, profile)?;
            Ok(Arc::new(AnthropicProvider::new(
                client,
                profile.base_url.clone(),
                key,
            )))
        }
        // NVIDIA / OpenAI / OpenAI-compatible / custom over the chat-completions adapter. The key is
        // optional: a keyless local endpoint (Ollama / LM Studio) builds with `None`, omitting the header.
        _ => {
            let key = optional_key(credential, profile)?;
            Ok(Arc::new(OpenAiProvider::new(
                client,
                profile.base_url.clone(),
                key,
                thinking,
                effort,
            )))
        }
    }
}

/// Build the embeddings adapter for `profile` + `credential` + `model`. Only the OpenAI-compatible
/// endpoint exposes embeddings; an Anthropic profile fails fast so the caller degrades to keyword recall
/// rather than issuing a request that endpoint cannot serve.
pub fn build_embedding_provider(
    client: reqwest::Client,
    profile: &ProviderProfile,
    credential: Credential,
    model: String,
) -> Result<Arc<dyn EmbeddingProvider>, AgentError> {
    if model.trim().is_empty() {
        return Err(AgentError::Provider(
            "embeddings model is empty; set [embeddings].model in ~/.kiri/config.toml".to_string(),
        ));
    }
    if profile.kind == ProviderKind::Anthropic {
        return Err(AgentError::Provider(format!(
            "provider '{}' (Anthropic) exposes no embeddings endpoint; point [embeddings].provider at an OpenAI-compatible provider",
            profile.id
        )));
    }
    let key = optional_key(credential, profile)?;
    Ok(Arc::new(OpenAiEmbeddingProvider::new(
        client,
        profile.base_url.clone(),
        key,
        model,
    )))
}

/// The legacy/CI env var an API-key provider can be primed from, by vendor plus a generic per-id form.
/// A migration aid (and the live-`/provider`-switch fallback) so a provider whose key lives in an env
/// var works without first storing it in the keyring. Filters empties per candidate so a set-but-blank
/// generic var does not shadow a real vendor var.
pub fn api_key_from_env(profile: &ProviderProfile) -> Option<Secret> {
    let generic = generic_env_key(profile);
    let vendor: &[&str] = match profile.kind {
        ProviderKind::Nvidia => &["NVIDIA_API_KEY"],
        ProviderKind::Openai => &["OPENAI_API_KEY"],
        ProviderKind::Anthropic => &["ANTHROPIC_API_KEY"],
        ProviderKind::OpenAiCompatible | ProviderKind::Custom => &[],
    };
    std::iter::once(generic.as_str())
        .chain(vendor.iter().copied())
        .find_map(|key| std::env::var(key).ok().and_then(secret_from_env_value))
}

/// Treat a blank value as absent (so a set-but-blank var never shadows a real one) and wrap a real value
/// in a [`Secret`], so the env key lives only inside zeroized, Debug-redacted memory — never a plain
/// `String` the caller must remember to wrap.
fn secret_from_env_value(value: String) -> Option<Secret> {
    if value.trim().is_empty() {
        None
    } else {
        Some(Secret::new(value))
    }
}

/// The generic per-provider env var name (`KIRI_<ID>_API_KEY`).
pub fn generic_env_key(profile: &ProviderProfile) -> String {
    format!(
        "KIRI_{}_API_KEY",
        profile.id.to_ascii_uppercase().replace('-', "_")
    )
}

/// Extract a *required* API key, failing if the credential is absent or OAuth. Used by the Anthropic
/// branch (no anonymous mode) — a `None` credential here is a configuration error.
fn api_key_of(credential: Credential, profile: &ProviderProfile) -> Result<Secret, AgentError> {
    match credential {
        Credential::ApiKey { key } => Ok(key),
        Credential::None => Err(AgentError::Provider(format!(
            "provider '{}' requires an API key but none is configured",
            profile.id
        ))),
        Credential::Oauth(_) => Err(AgentError::Provider(format!(
            "provider '{}' has an OAuth credential, but Kiri only supports API-key credentials",
            profile.id
        ))),
    }
}

/// Extract an *optional* API key for the OpenAI-compatible adapters: `None` for a keyless endpoint
/// (the adapter omits `Authorization`), the key for an API-key credential, and a hard error for OAuth.
fn optional_key(
    credential: Credential,
    profile: &ProviderProfile,
) -> Result<Option<Secret>, AgentError> {
    match credential {
        Credential::None => Ok(None),
        Credential::ApiKey { key } => Ok(Some(key)),
        Credential::Oauth(_) => Err(AgentError::Provider(format!(
            "provider '{}' has an OAuth credential, but Kiri only supports API-key credentials",
            profile.id
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_provider, secret_from_env_value};
    use crate::shared::kernel::provider::{
        AuthMethod, Credential, Effort, OauthTokens, ProviderKind, ProviderProfile, Secret,
    };

    fn profile(id: &str, kind: ProviderKind, auth: AuthMethod, model: &str) -> ProviderProfile {
        ProviderProfile {
            id: id.to_string(),
            kind,
            base_url: "https://example.test/v1".to_string(),
            model: model.to_string(),
            models: vec![],
            auth,
        }
    }

    fn api_key() -> Credential {
        Credential::ApiKey {
            key: Secret::new("k"),
        }
    }

    fn oauth() -> Credential {
        Credential::Oauth(OauthTokens {
            access: Secret::new("a"),
            refresh: Secret::new("r"),
            expires_at_ms: 0,
            account_id: None,
        })
    }

    #[test]
    fn selects_openai_compatible_for_api_key_kinds() {
        let client = reqwest::Client::new();
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        assert!(build_provider(client, &p, api_key(), true, Effort::High).is_ok());
    }

    #[test]
    fn selects_anthropic_for_api_key() {
        let client = reqwest::Client::new();
        let p = profile("claude", ProviderKind::Anthropic, AuthMethod::ApiKey, "m");
        assert!(build_provider(client, &p, api_key(), true, Effort::High).is_ok());
    }

    #[test]
    fn bails_for_subscription_oauth() {
        // Subscription OAuth is intentionally unsupported for both vendors; it must fail fast, not
        // silently fall through to an adapter.
        let client = reqwest::Client::new();
        let anthropic_oauth = profile("claude", ProviderKind::Anthropic, AuthMethod::Oauth, "m");
        assert!(
            build_provider(
                client.clone(),
                &anthropic_oauth,
                oauth(),
                true,
                Effort::High
            )
            .is_err()
        );
        let openai_oauth = profile("gpt", ProviderKind::Openai, AuthMethod::Oauth, "m");
        assert!(build_provider(client, &openai_oauth, oauth(), true, Effort::High).is_err());
    }

    #[test]
    fn bails_on_oauth_credential_for_api_key_kind() {
        let client = reqwest::Client::new();
        let p = profile("x", ProviderKind::OpenAiCompatible, AuthMethod::ApiKey, "m");
        assert!(build_provider(client, &p, oauth(), true, Effort::High).is_err());
    }

    #[test]
    fn bails_on_empty_model() {
        let client = reqwest::Client::new();
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "");
        assert!(build_provider(client, &p, api_key(), true, Effort::High).is_err());
    }

    #[test]
    fn builds_keyless_openai_compatible_and_custom() {
        // The reported LM Studio / Ollama case: a generic compatible (or custom) endpoint with no key.
        let client = reqwest::Client::new();
        for kind in [ProviderKind::OpenAiCompatible, ProviderKind::Custom] {
            let p = profile("local", kind, AuthMethod::None, "m");
            assert!(
                build_provider(client.clone(), &p, Credential::None, true, Effort::High).is_ok(),
                "{kind:?} keyless should build"
            );
        }
    }

    #[test]
    fn builds_keyed_openai_compatible() {
        // A remote OpenAI-compatible like OpenRouter still needs a key — presence of the key decides.
        let client = reqwest::Client::new();
        let p = profile(
            "openrouter",
            ProviderKind::OpenAiCompatible,
            AuthMethod::ApiKey,
            "m",
        );
        assert!(build_provider(client, &p, api_key(), true, Effort::High).is_ok());
    }

    #[test]
    fn rejects_keyless_vendor_kinds() {
        // Vendor endpoints have no anonymous mode; a keyless vendor profile must fail fast.
        let client = reqwest::Client::new();
        for kind in [
            ProviderKind::Nvidia,
            ProviderKind::Openai,
            ProviderKind::Anthropic,
        ] {
            let p = profile("vendor", kind, AuthMethod::None, "m");
            assert!(
                build_provider(client.clone(), &p, Credential::None, true, Effort::High).is_err(),
                "{kind:?} keyless should be rejected"
            );
        }
    }

    #[test]
    fn rejects_unrecognized_auth_method() {
        // A forward-version auth value leaves the provider inert rather than building keyless by accident.
        let client = reqwest::Client::new();
        let p = profile(
            "future",
            ProviderKind::OpenAiCompatible,
            AuthMethod::Unknown("magic".to_string()),
            "m",
        );
        assert!(build_provider(client, &p, Credential::None, true, Effort::High).is_err());
    }

    #[test]
    fn api_key_from_env_wraps_in_secret() {
        // The crate forbids `unsafe`, and edition-2024 `std::env::set_var` is unsafe, so the env read in
        // `api_key_from_env` cannot be driven in a test. Its only logic beyond the lookup is the pure
        // `secret_from_env_value` rule, asserted here: a real value becomes a Secret that exposes the
        // value yet redacts its Debug, while a blank value is treated as absent.
        let secret = secret_from_env_value("env-secret-value".to_string())
            .expect("a non-empty env value must wrap into Some(Secret)");
        assert_eq!(secret.expose(), "env-secret-value");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert!(
            secret_from_env_value("   ".to_string()).is_none(),
            "a blank env value must be treated as absent"
        );
    }
}
