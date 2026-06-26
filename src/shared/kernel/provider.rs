//! Cross-cutting provider primitives: which vendor/protocol a provider speaks, how it authenticates,
//! the reasoning effort, the (non-secret) configured profile, and the secret credential material.
//! Shared by `shared/infra/config` (reads them from TOML), the `provider` module (selects the adapter
//! and authenticates), and `tui` (the `/provider` wizard) — so they live in the kernel, like
//! [`super::approval_mode::ApprovalMode`], to keep the dependency direction one-way.

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The wire protocol + vendor a provider speaks. Together with [`AuthMethod`] it selects the concrete
/// adapter at the composition root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// NVIDIA's hosted OpenAI-compatible chat-completions endpoint (the default).
    Nvidia,
    /// Any other OpenAI-compatible chat-completions endpoint.
    OpenAiCompatible,
    /// OpenAI proper: chat-completions at `api.openai.com` with an API key.
    Openai,
    /// Anthropic Messages API, authenticated with `x-api-key`.
    Anthropic,
    /// A user-defined OpenAI-compatible endpoint (arbitrary base URL).
    Custom,
}

impl ProviderKind {
    /// The default base URL for this kind, used to seed a new profile in the `/provider` wizard. Empty
    /// for kinds whose endpoint the user must supply.
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::Nvidia => "https://integrate.api.nvidia.com/v1",
            ProviderKind::Openai => "https://api.openai.com/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAiCompatible | ProviderKind::Custom => "",
        }
    }
}

/// How a provider authenticates. `ApiKey` is the only wired method. `Oauth` is a modeled extension
/// point, not implemented: subscription OAuth (Claude Pro/Max, ChatGPT Plus/Pro) is intentionally
/// unsupported because the vendors restrict those tokens to their own clients (see the provider-auth
/// ADR); it is kept in the type so a future sanctioned flow can slot in without a schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    ApiKey,
    Oauth,
}

/// Reasoning / output effort. A provider-agnostic dial each adapter maps to its native parameter
/// (OpenAI-compatible `reasoning_effort` / nemotron `thinking`, Anthropic `output_config.effort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// No extended reasoning.
    Off,
    Low,
    Medium,
    #[default]
    High,
    Xhigh,
    Max,
}

/// A configured provider — everything non-secret needed to talk to it. The catalog the user selects
/// among; persisted in the TOML config. The secret material lives separately in a [`Credential`]
/// (keyring / 0600 file), keyed by [`ProviderProfile::id`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    /// Stable id; the map key in `[providers.<id>]`. Not serialized in the table body.
    #[serde(skip)]
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// The active model id sent on each turn.
    pub model: String,
    /// The catalog the `/models` picker offers (includes `model`); may be empty.
    #[serde(default)]
    pub models: Vec<String>,
    pub auth: AuthMethod,
}

/// The secret material for a provider, stored in the OS keyring (or a 0600 fallback file) as JSON.
/// Never written to the TOML config and never logged. Refresh tokens persist here; short-lived access
/// tokens are also persisted so a restarted session can refresh without re-login.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Credential {
    ApiKey { key: Secret },
    Oauth(OauthTokens),
}

/// An OAuth token set. `expires_at_ms` is epoch milliseconds; the auth layer refreshes proactively
/// before it lapses. `account_id` is required by the OpenAI/Codex backend (extracted from the id token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthTokens {
    pub access: Secret,
    pub refresh: Secret,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// A secret string: zeroized on drop and redacted in `Debug`, so it never lands in a log or transcript.
/// It serializes its inner value because the only sink is the OS keyring / a 0600 credentials file.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Borrow the underlying secret. Callers must only pass it to an auth header or token endpoint —
    /// never to a logger, the transcript, or an error message.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer).map_err(D::Error::custom)?;
        Ok(Secret::new(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        let s = Secret::new("super-secret-key");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(s.expose(), "super-secret-key");
    }

    #[test]
    fn credential_api_key_round_trips_as_json() {
        let cred = Credential::ApiKey {
            key: Secret::new("sk-abc"),
        };
        let json = serde_json::to_string(&cred).unwrap();
        assert!(json.contains("\"type\":\"api-key\""));
        assert!(json.contains("sk-abc"));
        let back: Credential = serde_json::from_str(&json).unwrap();
        match back {
            Credential::ApiKey { key } => assert_eq!(key.expose(), "sk-abc"),
            other => panic!("expected api-key, got {other:?}"),
        }
    }

    #[test]
    fn credential_oauth_round_trips_as_json() {
        let cred = Credential::Oauth(OauthTokens {
            access: Secret::new("at"),
            refresh: Secret::new("rt"),
            expires_at_ms: 1_700_000_000_000,
            account_id: Some("acc-1".into()),
        });
        let json = serde_json::to_string(&cred).unwrap();
        assert!(json.contains("\"type\":\"oauth\""));
        let back: Credential = serde_json::from_str(&json).unwrap();
        match back {
            Credential::Oauth(t) => {
                assert_eq!(t.access.expose(), "at");
                assert_eq!(t.refresh.expose(), "rt");
                assert_eq!(t.expires_at_ms, 1_700_000_000_000);
                assert_eq!(t.account_id.as_deref(), Some("acc-1"));
            }
            other => panic!("expected oauth, got {other:?}"),
        }
    }

    #[test]
    fn effort_serde_uses_lowercase_labels() {
        assert_eq!(serde_json::to_string(&Effort::Xhigh).unwrap(), "\"xhigh\"");
        let e: Effort = serde_json::from_str("\"max\"").unwrap();
        assert_eq!(e, Effort::Max);
    }
}
