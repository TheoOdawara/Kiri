//! Selects and constructs the concrete provider adapter from a [`ProviderProfile`] and its
//! [`Credential`]. Shared by the composition root (`app::wire`, initial construction) and the TUI
//! runtime (a live `/provider`/`/effort` swap), so the `(kind, auth)` → adapter decision lives in one
//! place. Returns [`AgentError`] (not `anyhow`) since both callers are inside the engine boundary.

use std::sync::Arc;

use super::anthropic::provider::AnthropicProvider;
use super::openai::provider::OpenAiProvider;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{
    AuthMethod, Credential, Effort, ProviderKind, ProviderProfile, Secret,
};

/// Construct the provider adapter for `profile` + `credential`. Two adapters cover every supported
/// provider, all by API key: the Anthropic Messages API adapter for `Anthropic`, and the
/// OpenAI-compatible chat-completions adapter for NVIDIA, generic compatible endpoints, custom
/// endpoints, and OpenAI proper. Subscription OAuth (Claude Pro/Max, ChatGPT Plus/Pro) is intentionally
/// unsupported — the vendors restrict those tokens to their own clients, so it would require
/// impersonation that risks the user's account (see the provider-auth ADR); an `Oauth` profile fails
/// fast with that rationale.
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
    match (profile.kind, profile.auth) {
        (ProviderKind::Anthropic, AuthMethod::Oauth) => Err(AgentError::Provider(format!(
            "provider '{}' uses Anthropic subscription OAuth, which Kiri does not support — Anthropic restricts Pro/Max OAuth tokens to its own clients. Configure an Anthropic Console API key instead.",
            profile.id
        ))),
        (ProviderKind::Openai, AuthMethod::Oauth) => Err(AgentError::Provider(format!(
            "provider '{}' uses ChatGPT subscription OAuth, which Kiri does not support — a ChatGPT subscription does not include API access. Configure a platform.openai.com API key instead.",
            profile.id
        ))),
        (ProviderKind::Anthropic, AuthMethod::ApiKey) => {
            let key = api_key_of(credential, profile)?;
            Ok(Arc::new(AnthropicProvider::new(
                client,
                profile.base_url.clone(),
                key.expose().to_string(),
            )))
        }
        _ => {
            let key = api_key_of(credential, profile)?;
            Ok(Arc::new(OpenAiProvider::new(
                client,
                profile.base_url.clone(),
                key.expose().to_string(),
                thinking,
                effort,
            )))
        }
    }
}

/// The legacy/CI env var an API-key provider can be primed from, by vendor plus a generic per-id form.
/// A migration aid (and the live-`/provider`-switch fallback) so a provider whose key lives in an env
/// var works without first storing it in the keyring. Filters empties per candidate so a set-but-blank
/// generic var does not shadow a real vendor var.
pub fn api_key_from_env(profile: &ProviderProfile) -> Option<String> {
    let generic = generic_env_key(profile);
    let vendor: &[&str] = match profile.kind {
        ProviderKind::Nvidia => &["NVIDIA_API_KEY"],
        ProviderKind::Openai => &["OPENAI_API_KEY"],
        ProviderKind::Anthropic => &["ANTHROPIC_API_KEY"],
        ProviderKind::OpenAiCompatible | ProviderKind::Custom => &[],
    };
    std::iter::once(generic.as_str())
        .chain(vendor.iter().copied())
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

/// The generic per-provider env var name (`KIRI_<ID>_API_KEY`).
pub fn generic_env_key(profile: &ProviderProfile) -> String {
    format!(
        "KIRI_{}_API_KEY",
        profile.id.to_ascii_uppercase().replace('-', "_")
    )
}

/// Extract the API key from a credential, failing if the profile somehow carries an OAuth credential
/// (Kiri only supports API-key auth). Shared by every adapter branch.
fn api_key_of(credential: Credential, profile: &ProviderProfile) -> Result<Secret, AgentError> {
    match credential {
        Credential::ApiKey { key } => Ok(key),
        Credential::Oauth(_) => Err(AgentError::Provider(format!(
            "provider '{}' has an OAuth credential, but Kiri only supports API-key credentials",
            profile.id
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::build_provider;
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
}
