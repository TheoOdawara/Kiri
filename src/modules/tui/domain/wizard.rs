use zeroize::Zeroize;

use crate::shared::kernel::provider::ProviderKind;

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
    /// True when the wizard is the first-run onboarding flow (welcome framing; cancelling keeps the
    /// submit gate up instead of stranding a credential-less app).
    pub onboarding: bool,
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
            .field("onboarding", &self.onboarding)
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
            onboarding: false,
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

    /// The field the current step edits, or `None` on the `Kind` step.
    fn field_mut(&mut self) -> Option<&mut String> {
        match self.step {
            WizardStep::Kind => None,
            WizardStep::ProviderId => Some(&mut self.id),
            WizardStep::BaseUrl => Some(&mut self.base_url),
            WizardStep::Model => Some(&mut self.model),
            WizardStep::ExtraModels => Some(&mut self.extra_models),
            WizardStep::ApiKey => Some(&mut self.api_key),
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
        let len = WIZARD_KINDS.len() as i32;
        self.kind_selected = (self.kind_selected as i32 + delta).rem_euclid(len) as usize;
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
}
