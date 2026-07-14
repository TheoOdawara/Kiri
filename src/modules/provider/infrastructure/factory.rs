use std::sync::Arc;

use super::anthropic::provider::AnthropicProvider;
use super::openai::embeddings::OpenAiEmbeddingProvider;
use super::openai::provider::OpenAiProvider;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::modules::provider::application::secret_store::SecretStore;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{
    AuthMethod, Credential, Effort, ProviderKind, ProviderProfile, Secret,
};

/// Subscription OAuth is intentionally unsupported: the vendors restrict those tokens to their own
/// clients, so using them would mean impersonation that risks the user's account (ADR 0011).
pub fn build_provider(
    client: reqwest::Client,
    profile: &ProviderProfile,
    credential: Credential,
    thinking: bool,
    effort: Effort,
) -> Result<Arc<dyn CompletionProvider>, AgentError> {
    // A blank model would otherwise surface as an opaque provider 400 on the first turn.
    if profile.model.trim().is_empty() {
        return Err(AgentError::Provider(format!(
            "provider '{}' has no model configured; set its `model` in ~/.kiri/config.toml (NVIDIA users can export NVIDIA_MODEL for the default provider)",
            profile.id
        )));
    }
    match (profile.kind, &profile.auth) {
        // Without this arm an unrecognized auth would fall through to the OpenAI adapter's catch-all.
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
        // Vendor endpoints have no anonymous mode, so a keyless vendor profile — hand-edited or synced —
        // fails fast instead of issuing unauthenticated requests that 401 with a worse message.
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
                effective_thinking(profile, thinking),
                effort,
            )))
        }
        // Everything else over chat-completions; a keyless local endpoint builds with `None`.
        _ => {
            let key = optional_key(credential, profile)?;
            Ok(Arc::new(OpenAiProvider::new(
                client,
                profile.base_url.clone(),
                key,
                profile.kind,
                effective_thinking(profile, thinking),
                effort,
                profile.thinking_style,
            )))
        }
    }
}

/// Per-profile `thinking` overrides the kind's default, and the global toggle gates both.
fn effective_thinking(profile: &ProviderProfile, thinking: bool) -> bool {
    profile
        .thinking
        .unwrap_or_else(|| profile.kind.thinking_default())
        && thinking
}

/// An Anthropic profile fails fast, so the caller degrades to keyword recall rather than issue a request
/// that endpoint cannot serve.
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

/// Empties are filtered per candidate, so a set-but-blank generic var cannot shadow a real vendor one.
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

/// Single-sources the credential *policy* while leaving reporting and absent-handling to each caller,
/// so boot and the live `/provider` swap cannot drift.
pub enum CredentialResolution {
    /// The secret store was never consulted.
    Keyless,
    Stored(Credential),
    /// `persisted` carries the best-effort `secrets.set` outcome so a caller surfaces a persist failure
    /// instead of swallowing it (ERR-02).
    Imported {
        credential: Credential,
        persisted: Result<(), AgentError>,
    },
    /// `KIRI_NO_KEY_IMPORT`: the key serves this session only and the store is never written, so a CI
    /// invocation leaves no durable copy behind (SEC-07).
    ImportedSessionOnly {
        credential: Credential,
    },
    /// A first-run signal, never a fatal abort.
    Absent,
}

/// Never logs the secret.
pub fn resolve_credential(
    profile: &ProviderProfile,
    secrets: &dyn SecretStore,
) -> Result<CredentialResolution, AgentError> {
    resolve_credential_with_env(profile, secrets, api_key_from_env, no_key_import_opt_out())
}

/// `1`/`true`, case-insensitive.
fn no_key_import_opt_out() -> bool {
    std::env::var("KIRI_NO_KEY_IMPORT")
        .map(|v| {
            let v = v.trim();
            v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// The env lookup and the opt-out are injected because the crate forbids `unsafe`, so a test cannot
/// call `set_var` to drive the import path.
fn resolve_credential_with_env(
    profile: &ProviderProfile,
    secrets: &dyn SecretStore,
    env_key: impl Fn(&ProviderProfile) -> Option<Secret>,
    no_import: bool,
) -> Result<CredentialResolution, AgentError> {
    // `profile.auth` recorded the key-presence decision at save time, so a stale key left by a prior
    // keyed config of this id must be ignored, not resurrected.
    if profile.auth == AuthMethod::None {
        return Ok(CredentialResolution::Keyless);
    }
    if let Some(credential) = secrets.get(&profile.id)? {
        return Ok(CredentialResolution::Stored(credential));
    }
    // An OAuth or forward-version auth must never import a bare env key.
    if profile.auth == AuthMethod::ApiKey
        && let Some(key) = env_key(profile)
    {
        let credential = Credential::ApiKey { key };
        if no_import {
            return Ok(CredentialResolution::ImportedSessionOnly { credential });
        }
        // Best-effort, so later sessions need no env var. Returned rather than swallowed.
        let persisted = secrets.set(&profile.id, &credential);
        return Ok(CredentialResolution::Imported {
            credential,
            persisted,
        });
    }
    Ok(CredentialResolution::Absent)
}

/// A blank value is absent, so a set-but-blank var never shadows a real one. Returning a [`Secret`]
/// keeps the key out of any plain `String` a caller must remember to wrap.
fn secret_from_env_value(value: String) -> Option<Secret> {
    if value.trim().is_empty() {
        None
    } else {
        Some(Secret::new(value))
    }
}

pub fn generic_env_key(profile: &ProviderProfile) -> String {
    format!(
        "KIRI_{}_API_KEY",
        profile.id.to_ascii_uppercase().replace('-', "_")
    )
}

/// The Anthropic branch has no anonymous mode, so a `None` credential is a configuration error here.
fn api_key_of(credential: Credential, profile: &ProviderProfile) -> Result<Secret, AgentError> {
    match optional_key(credential, profile)? {
        Some(key) => Ok(key),
        None => Err(AgentError::Provider(format!(
            "provider '{}' requires an API key but none is configured",
            profile.id
        ))),
    }
}

/// The single place OAuth is rejected; a keyless endpoint yields `None`.
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
    use super::{
        CredentialResolution, api_key_of, build_provider, effective_thinking,
        resolve_credential_with_env, secret_from_env_value,
    };
    use crate::modules::provider::application::secret_store::SecretStore;
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::provider::{
        AuthMethod, Credential, Effort, OauthTokens, ProviderKind, ProviderProfile, Secret,
    };

    /// `forbid_get` panics when consulted, proving the keyless short-circuit never reaches the store.
    struct FakeStore {
        stored: Option<Credential>,
        set_fails: bool,
        forbid_get: bool,
    }
    impl FakeStore {
        fn empty() -> Self {
            Self {
                stored: None,
                set_fails: false,
                forbid_get: false,
            }
        }
    }
    impl SecretStore for FakeStore {
        fn get(&self, _id: &str) -> Result<Option<Credential>, AgentError> {
            assert!(
                !self.forbid_get,
                "the secret store must not be consulted here"
            );
            Ok(self.stored.clone())
        }
        fn set(&self, _id: &str, _credential: &Credential) -> Result<(), AgentError> {
            if self.set_fails {
                Err(AgentError::Secret("store offline".to_string()))
            } else {
                Ok(())
            }
        }
        fn delete(&self, _id: &str) -> Result<(), AgentError> {
            Ok(())
        }
    }

    fn env_some(_p: &ProviderProfile) -> Option<Secret> {
        Some(Secret::new("env-key"))
    }
    fn env_none(_p: &ProviderProfile) -> Option<Secret> {
        None
    }

    #[test]
    fn resolves_keyless_to_keyless_resolution() {
        let store = FakeStore {
            forbid_get: true,
            ..FakeStore::empty()
        };
        let p = profile(
            "local",
            ProviderKind::OpenAiCompatible,
            AuthMethod::None,
            "m",
        );
        assert!(matches!(
            resolve_credential_with_env(&p, &store, env_some, false).unwrap(),
            CredentialResolution::Keyless
        ));
    }

    #[test]
    fn resolves_stored_credential() {
        let store = FakeStore {
            stored: Some(Credential::ApiKey {
                key: Secret::new("stored"),
            }),
            ..FakeStore::empty()
        };
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        match resolve_credential_with_env(&p, &store, env_none, false).unwrap() {
            CredentialResolution::Stored(Credential::ApiKey { key }) => {
                assert_eq!(key.expose(), "stored");
            }
            _ => panic!("expected Stored(ApiKey)"),
        }
    }

    #[test]
    fn resolves_env_import_and_reports_persist_ok() {
        let store = FakeStore::empty();
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        match resolve_credential_with_env(&p, &store, env_some, false).unwrap() {
            CredentialResolution::Imported {
                credential: Credential::ApiKey { key },
                persisted,
            } => {
                assert_eq!(key.expose(), "env-key");
                assert!(persisted.is_ok());
            }
            _ => panic!("expected Imported(ApiKey)"),
        }
    }

    #[test]
    fn no_import_uses_env_key_session_only_without_persisting() {
        // `set_fails` would surface as an Err if the store were written, proving no persist happened.
        let store = FakeStore {
            set_fails: true,
            ..FakeStore::empty()
        };
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        match resolve_credential_with_env(&p, &store, env_some, true).unwrap() {
            CredentialResolution::ImportedSessionOnly {
                credential: Credential::ApiKey { key },
            } => assert_eq!(key.expose(), "env-key"),
            _ => panic!("expected ImportedSessionOnly(ApiKey)"),
        }
    }

    #[test]
    fn import_persists_when_opt_out_is_unset() {
        // The mirror of the opt-out: the same env key persists when it is unset.
        let store = FakeStore::empty();
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        assert!(matches!(
            resolve_credential_with_env(&p, &store, env_some, false).unwrap(),
            CredentialResolution::Imported { .. }
        ));
    }

    #[test]
    fn resolves_env_import_surfaces_persist_failure() {
        let store = FakeStore {
            set_fails: true,
            ..FakeStore::empty()
        };
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        match resolve_credential_with_env(&p, &store, env_some, false).unwrap() {
            CredentialResolution::Imported { persisted, .. } => {
                assert!(matches!(persisted, Err(AgentError::Secret(_))));
            }
            _ => panic!("expected Imported"),
        }
    }

    #[test]
    fn resolves_absent_when_nothing_configured() {
        let store = FakeStore::empty();
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        assert!(matches!(
            resolve_credential_with_env(&p, &store, env_none, false).unwrap(),
            CredentialResolution::Absent
        ));
    }

    #[test]
    fn env_import_is_gated_on_api_key_auth() {
        // The env closure panics if called.
        let store = FakeStore::empty();
        let p = profile("gpt", ProviderKind::Openai, AuthMethod::Oauth, "m");
        let resolution = resolve_credential_with_env(
            &p,
            &store,
            |_| panic!("env must not be consulted for non-api-key auth"),
            false,
        )
        .unwrap();
        assert!(matches!(resolution, CredentialResolution::Absent));
    }

    fn profile(id: &str, kind: ProviderKind, auth: AuthMethod, model: &str) -> ProviderProfile {
        ProviderProfile {
            id: id.to_string(),
            kind,
            base_url: "https://example.test/v1".to_string(),
            model: model.to_string(),
            models: vec![],
            auth,
            thinking: None,
            thinking_style: Default::default(),
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
    fn api_key_of_rejects_oauth_once() {
        let p = profile("claude", ProviderKind::Anthropic, AuthMethod::ApiKey, "m");
        assert!(matches!(
            api_key_of(oauth(), &p),
            Err(AgentError::Provider(_))
        ));
        assert!(matches!(
            api_key_of(Credential::None, &p),
            Err(AgentError::Provider(_))
        ));
        assert_eq!(api_key_of(api_key(), &p).unwrap().expose(), "k");
    }

    #[test]
    fn bails_for_subscription_oauth() {
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
    fn effective_thinking_falls_back_to_the_kind_default_when_the_profile_is_unset() {
        // `profile()` leaves `thinking: None`, so `kind.thinking_default()` decides.
        let nvidia = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        assert!(effective_thinking(&nvidia, true));
        let compatible = profile(
            "local",
            ProviderKind::OpenAiCompatible,
            AuthMethod::ApiKey,
            "m",
        );
        assert!(!effective_thinking(&compatible, true));
    }

    #[test]
    fn effective_thinking_profile_override_beats_the_kind_default() {
        let mut off_override = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        off_override.thinking = Some(false);
        assert!(
            !effective_thinking(&off_override, true),
            "an explicit Some(false) must beat Nvidia's on-by-default"
        );

        let mut on_override = profile(
            "local",
            ProviderKind::OpenAiCompatible,
            AuthMethod::ApiKey,
            "m",
        );
        on_override.thinking = Some(true);
        assert!(
            effective_thinking(&on_override, true),
            "an explicit Some(true) must beat OpenAiCompatible's off-by-default"
        );
    }

    #[test]
    fn effective_thinking_is_false_when_the_global_toggle_is_off() {
        let nvidia = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        assert!(!effective_thinking(&nvidia, false));
    }

    #[test]
    fn builds_keyless_openai_compatible_and_custom() {
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
        // A remote OpenAI-compatible like OpenRouter still needs a key: its presence decides, not the kind.
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
        // `api_key_from_env`'s lookup cannot be driven under a crate that forbids `unsafe`, so only its
        // pure rule, `secret_from_env_value`, is asserted here.
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
