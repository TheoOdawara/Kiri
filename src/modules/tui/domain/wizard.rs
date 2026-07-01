use zeroize::Zeroize;

use crate::modules::tui::domain::nav::wrapping_step;
use crate::shared::kernel::provider::{AuthMethod, ProviderKind, ProviderProfile};

/// The last row of the `/provider` picker — selecting it opens the add-provider wizard instead of
/// switching. A sentinel label, never a real provider id.
pub const ADD_PROVIDER_LABEL: &str = "+ adicionar novo provider";

/// The provider kinds the wizard offers, in display order. NVIDIA leads (the seeded default), so it is
/// preselected at first-run onboarding; the rest follow. Vendor kinds require an API key; the generic
/// OpenAI-compatible and custom kinds may be keyless (Ollama / LM Studio) — the typed key decides.
pub const WIZARD_KINDS: [ProviderKind; 5] = [
    ProviderKind::Nvidia,
    ProviderKind::Anthropic,
    ProviderKind::Openai,
    ProviderKind::OpenAiCompatible,
    ProviderKind::Custom,
];

/// The steps of the add-provider wizard, in order. `ProviderId` is shown only for the keyless-capable
/// kinds (OpenAI-compatible / custom), so the user can name several coexisting endpoints; vendor kinds
/// use a canonical id and skip it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Kind,
    ProviderId,
    BaseUrl,
    Model,
    ExtraModels,
    /// Boolean chooser: whether to enable thinking/reasoning for this provider.
    Thinking,
    ApiKey,
}

/// The add-provider wizard's accumulated state. Each text step edits its own field directly; the `Kind`
/// step moves `kind_selected`. The API key is redacted in `Debug` so it can never land in a log even
/// though `Model` derives `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderWizard {
    pub step: WizardStep,
    pub kind_selected: usize,
    /// The user-chosen provider id (keyless-capable kinds only; vendor kinds use a canonical token).
    pub id: String,
    pub base_url: String,
    pub model: String,
    pub extra_models: String,
    pub api_key: String,
    /// Whether thinking/reasoning is enabled for this provider. Toggled on the `Thinking` step;
    /// pre-set to `kind.thinking_default()` when the `Kind` step advances.
    pub thinking: bool,
    /// True when the wizard is the first-run onboarding flow (welcome framing; cancelling keeps the
    /// submit gate up instead of stranding a credential-less app).
    pub onboarding: bool,
    /// True when editing an existing provider. In this mode the `Kind` step is skipped, the fields
    /// are pre-populated, and a blank `api_key` on the final step means "keep the existing key".
    pub edit_mode: bool,
    /// True when the profile being edited already had an `AuthMethod::ApiKey` credential (set by
    /// `from_profile`, `false` for a fresh `new()` wizard). A keyless-capable kind (compatible/custom)
    /// has `key_required() == false`, so without this the finalize step could not tell "this provider
    /// never had a key" apart from "this provider has a key I'm keeping" when the field is left blank —
    /// collapsing `auth` to `None` and deleting the real stored key out from under the user.
    pub had_key: bool,
}

impl std::fmt::Debug for ProviderWizard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderWizard")
            .field("step", &self.step)
            .field("kind", &self.kind())
            .field("id", &self.id)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("extra_models", &self.extra_models)
            .field("api_key", &"***")
            .field("thinking", &self.thinking)
            .field("onboarding", &self.onboarding)
            .field("edit_mode", &self.edit_mode)
            .field("had_key", &self.had_key)
            .finish()
    }
}

impl ProviderWizard {
    pub fn new() -> Self {
        Self {
            step: WizardStep::Kind,
            kind_selected: 0,
            id: String::new(),
            base_url: String::new(),
            model: String::new(),
            extra_models: String::new(),
            api_key: String::new(),
            thinking: true,
            onboarding: false,
            edit_mode: false,
            had_key: false,
        }
    }

    /// Open the wizard in edit mode, pre-populated from an existing profile. The `Kind` step is
    /// skipped (kind is locked); `api_key` starts empty ("keep existing" unless the user types one).
    pub fn from_profile(profile: &ProviderProfile) -> Self {
        let kind_selected = WIZARD_KINDS
            .iter()
            .position(|k| *k == profile.kind)
            .unwrap_or(0);
        let extra_models = profile
            .models
            .iter()
            .filter(|m| *m != &profile.model)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            step: WizardStep::BaseUrl,
            kind_selected,
            id: profile.id.clone(),
            base_url: profile.base_url.clone(),
            model: profile.model.clone(),
            extra_models,
            api_key: String::new(),
            thinking: profile.thinking.unwrap_or(profile.kind.thinking_default()),
            onboarding: false,
            edit_mode: true,
            had_key: profile.auth == AuthMethod::ApiKey,
        }
    }

    /// The wizard opened at first run with no credential: the welcome framing, NVIDIA preselected
    /// (`kind_selected = 0`, the leading entry in [`WIZARD_KINDS`]). Built by mutating `new()` rather than
    /// struct-update syntax, which cannot move fields out of a `Drop` type.
    pub fn onboarding() -> Self {
        let mut wizard = Self::new();
        wizard.onboarding = true;
        wizard
    }

    /// The selected provider kind.
    pub fn kind(&self) -> ProviderKind {
        WIZARD_KINDS[self.kind_selected.min(WIZARD_KINDS.len() - 1)]
    }

    /// Whether the selected kind requires an API key (vendor) or may be keyless (compatible / custom).
    pub fn key_required(&self) -> bool {
        self.kind().requires_api_key()
    }

    /// The kind's canonical id token (the wizard id for a vendor kind, and the fallback for a blank
    /// keyless id).
    fn canonical_id(&self) -> &'static str {
        match self.kind() {
            ProviderKind::Nvidia => "nvidia",
            ProviderKind::Openai => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::Custom => "custom",
        }
    }

    /// The id a finished wizard gives its provider. Vendor kinds use the canonical token (re-adding one
    /// reconfigures it). Keyless-capable kinds use the user-typed `id`, sanitized to a stable `[a-z0-9_-]`
    /// token, so several compatible endpoints (e.g. a local LM Studio and a remote OpenRouter) can coexist;
    /// a blank id falls back to the canonical token.
    pub fn provider_id(&self) -> String {
        if self.key_required() {
            return self.canonical_id().to_string();
        }
        let sanitized: String = self
            .id
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let sanitized = sanitized.trim_matches('-').to_string();
        if sanitized.is_empty() {
            self.canonical_id().to_string()
        } else {
            sanitized
        }
    }

    /// The model catalog: the default model first, then the comma-separated extras (trimmed, de-duped,
    /// blanks dropped).
    pub fn models(&self) -> Vec<String> {
        let mut models = Vec::new();
        let model = self.model.trim();
        if !model.is_empty() {
            models.push(model.to_string());
        }
        for extra in self.extra_models.split(',') {
            let extra = extra.trim();
            if !extra.is_empty() && !models.iter().any(|m| m == extra) {
                models.push(extra.to_string());
            }
        }
        models
    }

    /// The field the current step edits, or `None` on the `Kind`/`Thinking` steps (both are choosers,
    /// not text fields).
    fn field_mut(&mut self) -> Option<&mut String> {
        match self.step {
            WizardStep::Kind | WizardStep::Thinking => None,
            WizardStep::ProviderId => Some(&mut self.id),
            WizardStep::BaseUrl => Some(&mut self.base_url),
            WizardStep::Model => Some(&mut self.model),
            WizardStep::ExtraModels => Some(&mut self.extra_models),
            WizardStep::ApiKey => Some(&mut self.api_key),
        }
    }

    /// Toggle the thinking flag (only meaningful on the `Thinking` step).
    pub fn toggle_thinking(&mut self) {
        if self.step == WizardStep::Thinking {
            self.thinking = !self.thinking;
        }
    }

    pub fn push_char(&mut self, c: char) {
        if let Some(field) = self.field_mut() {
            field.push(c);
        }
    }

    /// Append pasted text to the current field, dropping control characters (a pasted key often carries
    /// a trailing newline). A no-op on the `Kind` step. This is how an API key is pasted into the masked
    /// field instead of leaking into the plaintext composer.
    pub fn push_str(&mut self, text: &str) {
        if let Some(field) = self.field_mut() {
            field.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn backspace(&mut self) {
        if let Some(field) = self.field_mut() {
            field.pop();
        }
    }

    /// Move the kind highlight (only meaningful on the `Kind` step), wrapping.
    pub fn move_kind(&mut self, delta: i32) {
        if self.step != WizardStep::Kind {
            return;
        }
        self.kind_selected = wrapping_step(self.kind_selected, delta, WIZARD_KINDS.len());
    }

    /// Whether the thinking toggle is "sim" (up/index 0) or "não" (down/index 1). Used by the renderer.
    pub fn thinking_selected_index(&self) -> usize {
        if self.thinking { 0 } else { 1 }
    }
}

impl Default for ProviderWizard {
    fn default() -> Self {
        Self::new()
    }
}

/// Zeroize the API-key buffer when the wizard is dropped (cancel, reopen, or after finalize), so a key
/// the user typed but never submitted does not linger in freed memory. Matches the project's `Secret`/
/// `Zeroizing` discipline. (Reallocations during editing can still leave residue — inherent to a growable
/// buffer; the staged `Secret` zeroizes the submitted value.) `Drop` is compatible with the finalize
/// path, which extracts the key via `mem::take` rather than moving the field out.
impl Drop for ProviderWizard {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_debug_redacts_the_api_key() {
        let mut w = ProviderWizard::new();
        w.step = WizardStep::ApiKey;
        w.api_key = "sk-super-secret".to_string();
        let rendered = format!("{w:?}");
        assert!(
            !rendered.contains("sk-super-secret"),
            "the API key leaked in Debug: {rendered}"
        );
        assert!(rendered.contains("***"));
    }

    #[test]
    fn wizard_models_puts_the_default_first_and_dedupes_extras() {
        let mut w = ProviderWizard::new();
        w.model = "  m1 ".to_string();
        w.extra_models = "m2, m1 , ,m3".to_string();
        assert_eq!(w.models(), vec!["m1", "m2", "m3"]);
    }

    #[test]
    fn wizard_provider_id_is_the_kind_token() {
        let w = ProviderWizard::new(); // kind index 0 = NVIDIA (the leading, preselected entry)
        assert_eq!(w.provider_id(), "nvidia");
        assert_eq!(w.kind(), ProviderKind::Nvidia);
    }

    #[test]
    fn key_required_only_for_vendor_kinds() {
        let mut w = ProviderWizard::new();
        // WIZARD_KINDS: [Nvidia, Anthropic, Openai, OpenAiCompatible, Custom].
        for (idx, required) in [(0, true), (1, true), (2, true), (3, false), (4, false)] {
            w.kind_selected = idx;
            assert_eq!(w.key_required(), required, "kind {:?}", w.kind());
        }
    }

    #[test]
    fn provider_id_sanitizes_a_named_keyless_provider() {
        let mut w = ProviderWizard::new();
        w.kind_selected = 3; // OpenAiCompatible — keyless-capable, so the typed id is used
        w.id = "  My LM Studio! ".to_string();
        assert_eq!(w.provider_id(), "my-lm-studio");
        // A blank id falls back to the canonical kind token, never an empty id.
        w.id = "   ".to_string();
        assert_eq!(w.provider_id(), "openai-compatible");
        // A vendor kind ignores the id field entirely.
        w.kind_selected = 0;
        w.id = "ignored".to_string();
        assert_eq!(w.provider_id(), "nvidia");
    }

    #[test]
    fn wizard_kinds_lead_with_nvidia() {
        assert_eq!(WIZARD_KINDS.len(), 5);
        assert_eq!(WIZARD_KINDS[0], ProviderKind::Nvidia);
    }

    #[test]
    fn onboarding_constructor_sets_flag_and_preselects_nvidia() {
        let w = ProviderWizard::onboarding();
        assert!(w.onboarding);
        assert_eq!(w.kind_selected, 0);
        assert_eq!(w.kind(), ProviderKind::Nvidia);
        assert_eq!(w.step, WizardStep::Kind);
    }

    fn saved_profile(
        kind: ProviderKind,
        auth: crate::shared::kernel::provider::AuthMethod,
    ) -> ProviderProfile {
        ProviderProfile {
            id: "openrouter".to_string(),
            kind,
            base_url: "https://openrouter.ai/api/v1".to_string(),
            model: "m1".to_string(),
            models: vec!["m1".to_string(), "m2".to_string()],
            auth,
            thinking: None,
        }
    }

    #[test]
    fn from_profile_pre_populates_fields_in_edit_mode() {
        let profile = saved_profile(ProviderKind::OpenAiCompatible, AuthMethod::ApiKey);
        let w = ProviderWizard::from_profile(&profile);
        assert!(w.edit_mode);
        assert_eq!(w.step, WizardStep::BaseUrl);
        assert_eq!(w.id, "openrouter");
        assert_eq!(w.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(w.model, "m1");
        assert_eq!(w.extra_models, "m2");
        assert!(
            w.api_key.is_empty(),
            "the key field starts blank in edit mode"
        );
    }

    #[test]
    fn from_profile_captures_had_key_from_the_saved_auth_method() {
        let keyed = saved_profile(ProviderKind::OpenAiCompatible, AuthMethod::ApiKey);
        assert!(ProviderWizard::from_profile(&keyed).had_key);

        let keyless = saved_profile(ProviderKind::OpenAiCompatible, AuthMethod::None);
        assert!(!ProviderWizard::from_profile(&keyless).had_key);

        // A fresh (non-edit) wizard never had a key.
        assert!(!ProviderWizard::new().had_key);
    }
}
