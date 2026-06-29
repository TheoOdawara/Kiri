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
use crate::modules::provider::application::secret_store::SecretStore;
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

/// The outcome of resolving a provider's credential. Single-sources the *policy* (keyless short-circuit
/// → stored → one-time env import → absent) while leaving *reporting* and *absent-handling* to each
/// caller, so the composition root (`app::wire`) and the live `/provider` swap (`ProviderSwap`) cannot
/// drift. It lives here, beside `api_key_from_env`/`SecretStore`, because the env/keyring access it
/// orchestrates is a binary-edge credential-acquisition concern, not domain policy.
pub enum CredentialResolution {
    /// `auth = "none"` → [`Credential::None`]; the secret store is never consulted.
    Keyless,
    /// Found in the secret store.
    Stored(Credential),
    /// A one-time import from the legacy/CI env var. `persisted` carries the best-effort `secrets.set`
    /// outcome so *both* callers can surface a persist failure instead of swallowing it (ERR-02).
    Imported {
        credential: Credential,
        persisted: Result<(), AgentError>,
    },
    /// An env-var import the user opted out of persisting (`KIRI_NO_KEY_IMPORT`): the key is used for
    /// this session only, the secret store is never written, so a single run / CI invocation leaves no
    /// durable copy behind (SEC-07).
    ImportedSessionOnly { credential: Credential },
    /// Nothing configured (no stored key, no env var) — a first-run signal, never a fatal abort.
    Absent,
}

/// Resolve a provider's credential by the single security-sensitive rule shared by boot and the live
/// `/provider` switch: a keyless provider short-circuits (the store is never consulted); else a stored
/// credential; else, *only* for an [`AuthMethod::ApiKey`] provider, a one-time import from the env var
/// (with the best-effort persist outcome returned, never swallowed); else absent. Never logs the secret.
pub fn resolve_credential(
    profile: &ProviderProfile,
    secrets: &dyn SecretStore,
) -> Result<CredentialResolution, AgentError> {
    resolve_credential_with_env(profile, secrets, api_key_from_env, no_key_import_opt_out())
}

/// Whether the user opted out of persisting a first-run env-key import via `KIRI_NO_KEY_IMPORT`
/// (`1`/`true`, case-insensitive). Lets an env key drive a single session / CI run without writing a
/// durable copy to the keyring or the `0600` fallback (SEC-07).
fn no_key_import_opt_out() -> bool {
    std::env::var("KIRI_NO_KEY_IMPORT")
        .map(|v| {
            let v = v.trim();
            v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// The testable core of [`resolve_credential`]: the env lookup and the no-persist opt-out are injected
/// so the import path can be exercised without mutating process env (the crate forbids `unsafe`, so
/// `set_var` is unavailable in tests). Production passes [`api_key_from_env`] and
/// [`no_key_import_opt_out`].
fn resolve_credential_with_env(
    profile: &ProviderProfile,
    secrets: &dyn SecretStore,
    env_key: impl Fn(&ProviderProfile) -> Option<Secret>,
    no_import: bool,
) -> Result<CredentialResolution, AgentError> {
    // Keyless: the key-presence decision was recorded in `profile.auth` at save time, so never consult
    // the store/env and ignore any stale key left from a prior keyed config of this id.
    if profile.auth == AuthMethod::None {
        return Ok(CredentialResolution::Keyless);
    }
    if let Some(credential) = secrets.get(&profile.id)? {
        return Ok(CredentialResolution::Stored(credential));
    }
    // Gate the env import on ApiKey auth (the stricter convergence of the two former copies): an OAuth or
    // forward-version auth never imports a bare env key.
    if profile.auth == AuthMethod::ApiKey
        && let Some(key) = env_key(profile)
    {
        let credential = Credential::ApiKey { key };
        // SEC-07 opt-out: use the env key for this session only, never touching the store, so a single
        // run / CI invocation leaves no durable copy behind.
        if no_import {
            return Ok(CredentialResolution::ImportedSessionOnly { credential });
        }
        // Best-effort persist so later sessions need no env var; the Result is *returned*, not swallowed,
        // so each caller can surface a failure (boot via eprintln, the live swap via a transcript Notice).
        let persisted = secrets.set(&profile.id, &credential);
        return Ok(CredentialResolution::Imported {
            credential,
            persisted,
        });
    }
    Ok(CredentialResolution::Absent)
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

/// Extract a *required* API key by delegating to [`optional_key`] (which rejects OAuth in one place)
/// and mapping a keyless `None` to the missing-key error. Used by the Anthropic branch (no anonymous
/// mode) — a `None` credential here is a configuration error.
fn api_key_of(credential: Credential, profile: &ProviderProfile) -> Result<Secret, AgentError> {
    match optional_key(credential, profile)? {
        Some(key) => Ok(key),
        None => Err(AgentError::Provider(format!(
            "provider '{}' requires an API key but none is configured",
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
    use super::{
        CredentialResolution, api_key_of, build_provider, resolve_credential_with_env,
        secret_from_env_value,
    };
    use crate::modules::provider::application::secret_store::SecretStore;
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::provider::{
        AuthMethod, Credential, Effort, OauthTokens, ProviderKind, ProviderProfile, Secret,
    };

    /// A `SecretStore` double: a fixed stored credential (or none), a toggle to fail `set`, and a guard
    /// that panics if `get` is consulted (to prove the keyless short-circuit never reaches the store).
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
        // auth = none short-circuits to Keyless without ever consulting the store (forbid_get panics).
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
        // SEC-07: with the opt-out set, an env key resolves to ImportedSessionOnly and the store's `set`
        // is never reached — `set_fails` would surface as an Err if it were, proving no persist happened.
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
        // The mirror of the opt-out: without it, the same env key persists (Imported), locking that the
        // new branch only diverts behavior when KIRI_NO_KEY_IMPORT is set.
        let store = FakeStore::empty();
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        assert!(matches!(
            resolve_credential_with_env(&p, &store, env_some, false).unwrap(),
            CredentialResolution::Imported { .. }
        ));
    }

    #[test]
    fn resolves_env_import_surfaces_persist_failure() {
        // ERR-02 regression lock: a failed persist is *returned* in `Imported.persisted`, never swallowed.
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
        // A non-ApiKey auth must never import from env: pass an env closure that would panic if called.
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
        // The required-key extractor delegates to optional_key, so the single OAuth rejection arm now
        // lives in one place: OAuth and a keyless None both error, an API key passes through.
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
