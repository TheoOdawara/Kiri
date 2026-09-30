use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The wire protocol + vendor a provider speaks. Together with [`AuthMethod`] it selects the concrete
/// adapter at the composition root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Nvidia,
    /// The intuitive spellings are read as aliases so a hand-edited config still parses; serialization
    /// stays canonical.
    #[serde(alias = "openai-compatible", alias = "openaicompatible")]
    OpenAiCompatible,
    Openai,
    Anthropic,
    /// A user-defined OpenAI-compatible endpoint (arbitrary base URL).
    Custom,
}

impl ProviderKind {
    /// Seeds a new profile in the `/provider` wizard. Empty for kinds whose endpoint the user supplies.
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::Nvidia => "https://integrate.api.nvidia.com/v1",
            ProviderKind::Openai => "https://api.openai.com/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAiCompatible | ProviderKind::Custom => "",
        }
    }

    /// Vendor endpoints always mandate a key; generic/custom ones may be keyless (Ollama / LM Studio), so
    /// the key's presence decides their auth method. Shared by the wizard and the factory.
    pub fn requires_api_key(self) -> bool {
        match self {
            ProviderKind::Nvidia | ProviderKind::Openai | ProviderKind::Anthropic => true,
            ProviderKind::OpenAiCompatible | ProviderKind::Custom => false,
        }
    }

    /// NVIDIA uses `chat_template_kwargs.thinking`; OpenAI proper uses `reasoning_effort`.
    pub fn thinking_default(self) -> bool {
        matches!(self, ProviderKind::Nvidia | ProviderKind::Openai)
    }

    /// A maintained static table: no vendor API exposes this as discoverable metadata. Drives whether the
    /// `/provider` wizard shows its `Thinking` step at all. Compatible/custom kinds honor
    /// [`ThinkingStyle`] (default auto); native kinds ignore the style.
    pub fn thinking_capability(self, model: &str) -> ThinkingCapability {
        self.thinking_capability_with_style(model, ThinkingStyle::Auto)
    }

    pub fn thinking_capability_with_style(
        self,
        model: &str,
        style: ThinkingStyle,
    ) -> ThinkingCapability {
        match self {
            ProviderKind::Openai => ThinkingCapability::Discrete,
            ProviderKind::Anthropic => ThinkingCapability::Budget,
            ProviderKind::Nvidia => NvidiaFamily::classify(model).capability(),
            ProviderKind::OpenAiCompatible | ProviderKind::Custom => match style {
                ThinkingStyle::Off => ThinkingCapability::Unsupported,
                ThinkingStyle::ReasoningEffort => ThinkingCapability::Discrete,
                ThinkingStyle::ChatTemplate => {
                    if NvidiaFamily::classify(model).capability() == ThinkingCapability::Toggle {
                        ThinkingCapability::Toggle
                    } else {
                        ThinkingCapability::Unsupported
                    }
                }
                ThinkingStyle::Auto => generalist_thinking_capability(model),
            },
        }
    }
}

/// Optional wire strategy for OpenAI-compatible / custom endpoints. Native kinds ignore this field.
/// Documented in `docs/reference/model-thinking-parameters.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingStyle {
    /// Template kwargs for open-weights hybrid models; `reasoning_effort` for GPT/Grok-like ids;
    /// nothing for DeepSeek (unsafe to guess).
    #[default]
    Auto,
    /// Always OpenAI-style `reasoning_effort`.
    ReasoningEffort,
    /// Always family-keyed `chat_template_kwargs` (nothing if the family is unknown).
    ChatTemplate,
    /// Never send thinking-related fields.
    Off,
}

/// Generalist auto: only offer a thinking control when the model id matches a confirmed convention.
fn generalist_thinking_capability(model: &str) -> ThinkingCapability {
    if NvidiaFamily::classify(model).capability() == ThinkingCapability::Toggle {
        return ThinkingCapability::Toggle;
    }
    if is_openai_style_reasoning_model(model) {
        return ThinkingCapability::Discrete;
    }
    ThinkingCapability::Unsupported
}

/// GPT / Grok / o-series style ids that commonly accept `reasoning_effort` on OpenAI-compatible hosts.
pub(crate) fn is_openai_style_reasoning_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("gpt")
        || model.contains("grok")
        || model.contains("codex")
        || model.contains("o1")
        || model.contains("o3")
        || model.contains("o4")
}

pub(crate) fn is_deepseek_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("deepseek")
}

/// How a provider/model exposes reasoning ("thinking") control, if at all. See
/// [`ProviderKind::thinking_capability`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingCapability {
    /// The wizard skips asking about reasoning entirely.
    Unsupported,
    /// On/off, no granularity (an NVIDIA family using a boolean chat-template kwarg).
    Toggle,
    /// A discrete low/medium/high dial (OpenAI's `reasoning_effort`).
    Discrete,
    /// A token-budget dial (Anthropic's `thinking.budget_tokens`).
    Budget,
}

/// An NVIDIA-hosted model family, matched on the model id. One NIM endpoint fronts many families, each
/// with a different (or absent) reasoning-toggle convention. Also reused by the OpenAI-compatible
/// generalist path for the same template-kwargs keys. Never guess a wire shape — a wrong one
/// silently no-ops or 400s. Extend this only once a family's convention is confirmed against an official
/// reference page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NvidiaFamily {
    /// `chat_template_kwargs.thinking: bool`.
    Nemotron,
    /// `chat_template_kwargs.thinking: bool`, same key as Nemotron.
    Kimi,
    /// `chat_template_kwargs.enable_thinking: bool`.
    Qwen,
    /// `chat_template_kwargs.enable_thinking: bool` — the vLLM/SGLang convention NIM proxies, not
    /// Zhipu's own top-level `thinking: {type: "enabled"}` shape.
    Glm,
    /// `chat_template_kwargs.enable_thinking: bool` — Gemma **4** only (Vertex open models / vLLM).
    /// Gemma 3 and bare `gemma` stay [`Other`].
    Gemma,
    /// No reliable toggle. DeepSeek is worse than unverified: NIM is reported to hang on V4 reasoning
    /// models when `chat_template_kwargs` is absent, and vLLM ignores the toggle on `deepseek-r1-0528`.
    /// A guessed shape would reintroduce that instability.
    Other,
}

impl NvidiaFamily {
    pub(crate) fn classify(model: &str) -> Self {
        let model = model.to_ascii_lowercase();
        if model.contains("nemotron") {
            NvidiaFamily::Nemotron
        } else if model.contains("kimi") {
            NvidiaFamily::Kimi
        } else if model.contains("qwen") {
            NvidiaFamily::Qwen
        } else if model.contains("glm") {
            NvidiaFamily::Glm
        } else if is_gemma4_model(&model) {
            NvidiaFamily::Gemma
        } else {
            NvidiaFamily::Other
        }
    }

    fn capability(self) -> ThinkingCapability {
        match self {
            NvidiaFamily::Nemotron
            | NvidiaFamily::Kimi
            | NvidiaFamily::Qwen
            | NvidiaFamily::Glm
            | NvidiaFamily::Gemma => ThinkingCapability::Toggle,
            NvidiaFamily::Other => ThinkingCapability::Unsupported,
        }
    }
}

/// Gemma 4 only — not gemma-3 or bare `gemma`.
fn is_gemma4_model(model: &str) -> bool {
    model.contains("gemma-4") || model.contains("gemma4") || model.contains("gemma_4")
}

/// How a provider authenticates, selecting which credential the adapter sends. Serde is hand-written so
/// an unrecognized value maps to `Unknown` instead of failing — the forward-compatibility guarantee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    ApiKey,
    /// Keyless endpoints (Ollama / LM Studio): the adapter omits the `Authorization` header entirely.
    None,
    /// Modeled, never wired: subscription OAuth is unsupported because the vendors restrict those tokens
    /// to their own clients. Kept so a future sanctioned flow can slot in.
    Oauth,
    /// Written by a newer Kiri. Carries the original text, so reading a forward-version config never
    /// aborts the boot and rewriting it never corrupts the value; the factory leaves it inert.
    Unknown(String),
}

impl AuthMethod {
    pub fn as_wire(&self) -> &str {
        match self {
            AuthMethod::ApiKey => "api-key",
            AuthMethod::None => "none",
            AuthMethod::Oauth => "oauth",
            AuthMethod::Unknown(raw) => raw,
        }
    }
}

impl Serialize for AuthMethod {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_wire())
    }
}

impl<'de> Deserialize<'de> for AuthMethod {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "api-key" => AuthMethod::ApiKey,
            "none" => AuthMethod::None,
            "oauth" => AuthMethod::Oauth,
            _ => AuthMethod::Unknown(raw),
        })
    }
}

/// Reasoning / output effort. A provider-agnostic dial each adapter maps to its native parameter
/// (OpenAI-compatible `reasoning_effort` / nemotron `thinking`, Anthropic `thinking.budget_tokens`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Off,
    Low,
    Medium,
    #[default]
    High,
    Xhigh,
    Max,
}

impl Effort {
    /// Ascending, so the `/effort` picker's row index maps back to the level with no string round-trip.
    pub const ALL: [Effort; 6] = [
        Effort::Off,
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];

    /// Matches the serde form. Shown in the picker and the status line.
    pub fn label(self) -> &'static str {
        match self {
            Effort::Off => "off",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }

    /// `None` when effort is `Off`. Only meaningful for `ProviderKind::Openai`.
    pub fn as_openai_reasoning_effort(self) -> Option<&'static str> {
        match self {
            Effort::Off => None,
            Effort::Low => Some("low"),
            Effort::Medium => Some("medium"),
            Effort::High | Effort::Xhigh | Effort::Max => Some("high"),
        }
    }

    /// Kept under the adapter's `MAX_OUTPUT_TOKENS` (16_000) with headroom for the answer: the Messages
    /// API requires `1024 <= budget_tokens < max_tokens`. `None` when effort is `Off`.
    pub fn anthropic_budget_tokens(self) -> Option<u32> {
        match self {
            Effort::Off => None,
            Effort::Low => Some(1_024),
            Effort::Medium => Some(4_096),
            Effort::High => Some(8_192),
            Effort::Xhigh => Some(12_000),
            Effort::Max => Some(14_000),
        }
    }

    /// For adaptive thinking only (`AnthropicThinkingMode::AdaptiveOptIn`/`AdaptiveDefaultOn`).
    pub fn as_anthropic_output_effort(self) -> Option<&'static str> {
        (self != Effort::Off).then(|| self.label())
    }
}

/// Everything non-secret needed to talk to a provider; persisted in the TOML config. The secret material
/// lives separately in a [`Credential`] (a 0600 file), keyed by [`ProviderProfile::id`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    /// The map key in `[providers.<id>]`, so not serialized in the table body.
    #[serde(skip)]
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// The active model id sent on each turn.
    pub model: String,
    /// What the `/models` picker offers (includes `model`); may be empty.
    #[serde(default)]
    pub models: Vec<String>,
    pub auth: AuthMethod,
    /// `None` uses `kind.thinking_default()`; `Some(false)` disables it even for kinds enabling it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    /// Wire strategy for OpenAI-compatible / custom endpoints. Native kinds ignore this.
    /// Omit from TOML to use [`ThinkingStyle::Auto`].
    #[serde(default, skip_serializing_if = "ThinkingStyle::is_auto")]
    pub thinking_style: ThinkingStyle,
}

impl ThinkingStyle {
    fn is_auto(&self) -> bool {
        *self == ThinkingStyle::Auto
    }
}

/// Stored as JSON in the 0600 credentials file. Never written to the TOML config and never logged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Credential {
    ApiKey {
        key: Secret,
    },
    /// Synthesized by the composition root for a keyless provider; not normally persisted.
    None,
    Oauth(OauthTokens),
}

/// For the modeled-but-inert OAuth method (see [`AuthMethod::Oauth`]). `expires_at_ms` is epoch millis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthTokens {
    pub access: Secret,
    pub refresh: Secret,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// Zeroized on drop and redacted in `Debug`, so it never lands in a log or transcript. It serializes its
/// inner value because the only sink is the 0600 credentials file.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Only ever pass this to an auth header or token endpoint — never to a logger, the transcript, or
    /// an error message.
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
        let raw = String::deserialize(deserializer)?;
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
    fn secret_deserialize_preserves_native_error() {
        // SHARED-10: deserialize through `String::deserialize` directly — no custom re-wrap. A valid
        // string round-trips; a non-string surfaces the deserializer's own native type error.
        let secret: Secret = serde_json::from_str("\"sk-xyz\"").unwrap();
        assert_eq!(secret.expose(), "sk-xyz");
        let err = serde_json::from_str::<Secret>("42")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("string"),
            "expected a native type error, got {err}"
        );
    }

    #[test]
    fn effort_serde_uses_lowercase_labels() {
        assert_eq!(serde_json::to_string(&Effort::Xhigh).unwrap(), "\"xhigh\"");
        let e: Effort = serde_json::from_str("\"max\"").unwrap();
        assert_eq!(e, Effort::Max);
    }

    #[test]
    fn thinking_style_defaults_to_auto_and_omits_from_profile_when_auto() {
        let profile = ProviderProfile {
            id: "x".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "http://localhost/v1".into(),
            model: "m".into(),
            models: vec![],
            auth: AuthMethod::None,
            thinking: None,
            thinking_style: ThinkingStyle::Auto,
        };
        let json = serde_json::to_value(&profile).unwrap();
        assert!(
            json.get("thinking_style").is_none(),
            "auto must skip serialization so existing configs stay clean; got {json}"
        );
        let forced = ProviderProfile {
            thinking_style: ThinkingStyle::ChatTemplate,
            ..profile
        };
        let json = serde_json::to_value(&forced).unwrap();
        assert_eq!(json["thinking_style"], "chat-template");
    }

    #[test]
    fn auth_method_known_variants_round_trip() {
        for (value, wire) in [
            (AuthMethod::ApiKey, "\"api-key\""),
            (AuthMethod::None, "\"none\""),
            (AuthMethod::Oauth, "\"oauth\""),
        ] {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<AuthMethod>(wire).unwrap(), value);
        }
    }

    #[test]
    fn auth_method_unknown_round_trips_losslessly() {
        // A value written by a newer Kiri must deserialize without error (never aborting the boot) and
        // re-serialize byte-identically, so rewriting the config does not corrupt the original token.
        let back: AuthMethod = serde_json::from_str("\"future-method\"").unwrap();
        assert_eq!(back, AuthMethod::Unknown("future-method".to_string()));
        assert_eq!(serde_json::to_string(&back).unwrap(), "\"future-method\"");
    }

    #[test]
    fn credential_none_round_trips_as_typed_none() {
        let json = serde_json::to_string(&Credential::None).unwrap();
        assert!(json.contains("\"type\":\"none\""), "got {json}");
        assert!(matches!(
            serde_json::from_str::<Credential>(&json).unwrap(),
            Credential::None
        ));
    }

    #[test]
    fn provider_kind_accepts_intuitive_openai_compatible_aliases() {
        // A hand-edited config using the intuitive spelling must still parse to OpenAiCompatible.
        for token in [
            "open-ai-compatible",
            "openai-compatible",
            "openaicompatible",
        ] {
            let parsed: ProviderKind = serde_json::from_str(&format!("\"{token}\"")).unwrap();
            assert_eq!(parsed, ProviderKind::OpenAiCompatible, "token {token}");
        }
        // Serialization stays canonical (kebab-case), so written configs are unchanged.
        assert_eq!(
            serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap(),
            "\"open-ai-compatible\""
        );
    }

    #[test]
    fn requires_api_key_is_true_only_for_vendor_kinds() {
        assert!(ProviderKind::Nvidia.requires_api_key());
        assert!(ProviderKind::Openai.requires_api_key());
        assert!(ProviderKind::Anthropic.requires_api_key());
        assert!(!ProviderKind::OpenAiCompatible.requires_api_key());
        assert!(!ProviderKind::Custom.requires_api_key());
    }

    #[test]
    fn openai_and_anthropic_capability_ignore_the_model_string() {
        assert_eq!(
            ProviderKind::Openai.thinking_capability("gpt-5"),
            ThinkingCapability::Discrete
        );
        assert_eq!(
            ProviderKind::Anthropic.thinking_capability("claude-opus-4-8"),
            ThinkingCapability::Budget
        );
    }

    #[test]
    fn compatible_auto_offers_thinking_only_for_known_families() {
        assert_eq!(
            ProviderKind::OpenAiCompatible.thinking_capability("qwen/qwen3-32b"),
            ThinkingCapability::Toggle
        );
        assert_eq!(
            ProviderKind::OpenAiCompatible.thinking_capability("google/gemma-4-26b-a4b-it"),
            ThinkingCapability::Toggle
        );
        assert_eq!(
            ProviderKind::Custom.thinking_capability("openai/gpt-5"),
            ThinkingCapability::Discrete
        );
        assert_eq!(
            ProviderKind::OpenAiCompatible.thinking_capability("deepseek-ai/deepseek-r1"),
            ThinkingCapability::Unsupported
        );
        assert_eq!(
            ProviderKind::Custom.thinking_capability("gemma-3"),
            ThinkingCapability::Unsupported
        );
    }

    #[test]
    fn thinking_style_off_disables_compatible_capability() {
        assert_eq!(
            ProviderKind::OpenAiCompatible
                .thinking_capability_with_style("qwen3", ThinkingStyle::Off),
            ThinkingCapability::Unsupported
        );
        assert_eq!(
            ProviderKind::OpenAiCompatible
                .thinking_capability_with_style("anything", ThinkingStyle::ReasoningEffort),
            ThinkingCapability::Discrete
        );
    }

    #[test]
    fn nvidia_capability_is_keyed_on_the_model_family() {
        // Confirmed template-toggle families (incl. Gemma 4 via enable_thinking).
        for toggle_model in [
            "nvidia/llama-3.3-nemotron-super-49b-v1",
            "moonshotai/kimi-k2",
            "qwen/qwen3-235b-a22b",
            "zai-org/glm-4.5",
            "google/gemma-4-26b-a4b-it",
            "gemma4:26b",
        ] {
            assert_eq!(
                ProviderKind::Nvidia.thinking_capability(toggle_model),
                ThinkingCapability::Toggle,
                "expected Toggle for {toggle_model}"
            );
        }
        for unsupported_model in [
            // DeepSeek: a reported NIM hang (V4, absent chat_template_kwargs) and a vLLM issue (the
            // toggle not honored on r1-0528) make this "known unreliable", not just "unverified".
            "deepseek-ai/deepseek-v4",
            "deepseek-ai/deepseek-r1",
            "minimaxai/minimax-m1",
            // Gemma 3 / bare gemma: no confirmed enable_thinking convention in this table.
            "google/gemma-3-27b-it",
            "gemma",
            "some/unknown-model",
        ] {
            assert_eq!(
                ProviderKind::Nvidia.thinking_capability(unsupported_model),
                ThinkingCapability::Unsupported,
                "expected unsupported for {unsupported_model}"
            );
        }
    }

    #[test]
    fn anthropic_budget_tokens_is_none_when_off_and_rises_with_effort() {
        assert_eq!(Effort::Off.anthropic_budget_tokens(), None);
        let mut previous = 0;
        for effort in [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ] {
            let budget = effort
                .anthropic_budget_tokens()
                .expect("every non-Off level has a budget");
            assert!(budget >= 1_024, "{effort:?} budget below the API minimum");
            assert!(
                budget > previous,
                "{effort:?} budget must exceed the prior level"
            );
            previous = budget;
        }
    }

    #[test]
    fn anthropic_output_effort_is_none_when_off_and_matches_label_otherwise() {
        assert_eq!(Effort::Off.as_anthropic_output_effort(), None);
        for effort in [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ] {
            assert_eq!(effort.as_anthropic_output_effort(), Some(effort.label()));
        }
    }
}
