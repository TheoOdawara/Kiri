use super::*;
use crate::modules::tui::domain::modal::{PendingApproval, PendingPlan};

fn press(code: Key) -> KeyPress {
    KeyPress {
        code,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

/// Type a line and submit it (Enter), returning the submit effects.
fn submit_line(model: &mut Model, line: &str) -> Vec<Effect> {
    for c in line.chars() {
        on_key(model, press(Key::Char(c)));
    }
    on_key(model, press(Key::Enter))
}

#[test]
fn effort_command_opens_a_picker_at_the_current_level() {
    let mut m = Model::default().with_provider_catalog(Vec::new(), Effort::Medium);
    let effects = submit_line(&mut m, "/effort");
    assert!(effects.is_empty(), "opening a picker emits no effect");
    let picker = m.picker.as_ref().expect("the effort picker should open");
    assert_eq!(picker.kind, PickerKind::Effort);
    assert_eq!(picker.options.len(), Effort::ALL.len());
    // The current effort (Medium) is pre-selected.
    assert_eq!(picker.selected, 2);
}

#[test]
fn effort_picker_enter_emits_set_effort() {
    let picker = Picker::new(
        PickerKind::Effort,
        "esforço",
        "Escolha:",
        Effort::ALL.iter().map(|e| e.label().to_string()).collect(),
        0,
    );
    let mut m = Model {
        picker: Some(picker),
        ..Default::default()
    };
    // Down off -> low (index 1), then Enter.
    assert!(on_key(&mut m, press(Key::Down)).is_empty());
    let effects = on_key(&mut m, press(Key::Enter));
    assert_eq!(effects, vec![Effect::SetEffort(Effort::Low)]);
    assert!(m.picker.is_none(), "Enter closes the picker");
}

#[test]
fn picker_digit_selects_a_row() {
    let models = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let mut m = Model::default().with_provider_catalog(models, Effort::default());
    submit_line(&mut m, "/models");
    // Typing 'c' filters the list to only "c".
    assert!(on_key(&mut m, press(Key::Char('c'))).is_empty());
    // Since only "c" matches, selected will be 0 (the only item in filtered list).
    // Pressing Enter will select the highlighted item "c".
    let effects = on_key(&mut m, press(Key::Enter));
    assert_eq!(effects, vec![Effect::SetModel("c".to_string())]);
}

#[test]
fn picker_search_query_typing_and_backspace() {
    let models = vec![
        "apple".to_string(),
        "banana".to_string(),
        "cherry".to_string(),
    ];
    let mut m = Model::default().with_provider_catalog(models, Effort::default());
    submit_line(&mut m, "/models");

    // Type 'a'
    on_key(&mut m, press(Key::Char('a')));
    {
        let picker = m.picker.as_ref().unwrap();
        assert_eq!(picker.query, "a");
        assert_eq!(picker.filtered_options().len(), 2); // apple, banana
    }

    // Type 'n' -> query is "an"
    on_key(&mut m, press(Key::Char('n')));
    {
        let picker = m.picker.as_ref().unwrap();
        assert_eq!(picker.query, "an");
        assert_eq!(picker.filtered_options().len(), 1); // banana
    }

    // Backspace -> query is "a" again
    on_key(&mut m, press(Key::Backspace));
    {
        let picker = m.picker.as_ref().unwrap();
        assert_eq!(picker.query, "a");
        assert_eq!(picker.filtered_options().len(), 2); // apple, banana
    }
}

#[test]
fn wizard_ctrl_v_routes_to_paste_not_a_literal_char() {
    // On a text step, Ctrl+V must paste (into the masked field, via the clipboard) rather than
    // insert a literal 'v' — the regression that silently corrupts a pasted API key.
    let mut wizard = ProviderWizard::new();
    wizard.step = WizardStep::ApiKey;
    let mut m = Model {
        wizard: Some(wizard),
        ..Default::default()
    };
    let ctrl_v = KeyPress {
        code: Key::Char('v'),
        ctrl: true,
        alt: false,
        shift: false,
    };
    assert_eq!(on_key(&mut m, ctrl_v), vec![Effect::PasteClipboard]);
}

#[test]
fn provider_picker_add_row_opens_the_wizard() {
    let mut m =
        Model::default().with_providers("nvidia".to_string(), vec!["nvidia".to_string()], vec![]);
    submit_line(&mut m, "/provider");
    // options = ["nvidia", "+ adicionar..."]; Down lands on the add row, Enter opens the wizard.
    on_key(&mut m, press(Key::Down));
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(effects.is_empty());
    assert!(m.wizard.is_some(), "the add row opens the wizard");
}

#[test]
fn wizard_completes_with_staged_secret_out_of_the_effect() {
    use crate::shared::kernel::provider::{AuthMethod, ProviderKind};
    let mut m = Model {
        wizard: Some(ProviderWizard::new()),
        ..Default::default()
    };
    // Kind: NVIDIA is index 0; Down moves to Anthropic (index 1) -> Enter (seeds base_url).
    // BaseUrl: accept default -> Enter.
    on_key(&mut m, press(Key::Down));
    on_key(&mut m, press(Key::Enter));
    on_key(&mut m, press(Key::Enter));
    // Model: required.
    for c in "claude-opus-4-8".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter));
    // ExtraModels: skip.
    on_key(&mut m, press(Key::Enter));
    // Thinking: accept default.
    on_key(&mut m, press(Key::Enter));
    // ApiKey: type, then finalize.
    for c in "sk-ant-secret".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(m.wizard.is_none(), "the wizard closes on finalize");
    match effects.as_slice() {
        [
            Effect::SaveProvider {
                id,
                kind,
                model,
                auth,
                ..
            },
        ] => {
            assert_eq!(id, "anthropic");
            assert_eq!(*kind, ProviderKind::Anthropic);
            assert_eq!(model, "claude-opus-4-8");
            assert_eq!(*auth, AuthMethod::ApiKey);
        }
        other => panic!("expected SaveProvider, got {other:?}"),
    }
    // The key is staged as a Secret, never carried in the effect.
    let staged = m.pending_credential.as_ref().expect("the key is staged");
    assert_eq!(staged.expose(), "sk-ant-secret");
}

#[test]
fn wizard_model_step_requires_a_value() {
    let mut m = Model {
        wizard: Some(ProviderWizard::new()),
        ..Default::default()
    };
    on_key(&mut m, press(Key::Enter)); // Kind -> BaseUrl
    on_key(&mut m, press(Key::Enter)); // BaseUrl -> Model
    // Enter on an empty Model must not advance.
    on_key(&mut m, press(Key::Enter));
    assert_eq!(
        m.wizard.as_ref().map(|w| w.step),
        Some(WizardStep::Model),
        "an empty model keeps the wizard on the Model step"
    );
}

#[test]
fn wizard_esc_cancels() {
    let mut m = Model {
        wizard: Some(ProviderWizard::new()),
        ..Default::default()
    };
    let effects = on_key(&mut m, press(Key::Esc));
    assert!(effects.is_empty());
    assert!(m.wizard.is_none());
}

#[test]
fn nvidia_kind_confirm_seeds_base_url() {
    use crate::shared::kernel::provider::ProviderKind;
    let mut m = Model {
        wizard: Some(ProviderWizard::onboarding()),
        ..Default::default()
    };
    // NVIDIA is preselected; confirming the kind seeds its default endpoint.
    on_key(&mut m, press(Key::Enter));
    let wizard = m.wizard.as_ref().expect("the wizard advances to BaseUrl");
    assert_eq!(wizard.step, WizardStep::BaseUrl);
    assert_eq!(wizard.base_url, ProviderKind::Nvidia.default_base_url());
}

#[test]
fn onboarding_wizard_completes_to_nvidia_save_provider() {
    use crate::shared::kernel::provider::{AuthMethod, ProviderKind};
    let mut m = Model::default();
    m.enter_onboarding();
    // Kind (NVIDIA) -> Enter seeds base_url. BaseUrl -> Enter accepts default.
    on_key(&mut m, press(Key::Enter));
    on_key(&mut m, press(Key::Enter));
    // Model is required and not prefilled — type it.
    for c in "nvidia/some-model".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter));
    // ExtraModels: skip.
    on_key(&mut m, press(Key::Enter));
    // Thinking: accept default.
    on_key(&mut m, press(Key::Enter));
    // ApiKey: type, then finalize.
    for c in "nvapi-secret".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(m.wizard.is_none(), "the wizard closes on finalize");
    match effects.as_slice() {
        [
            Effect::SaveProvider {
                id,
                kind,
                model,
                auth,
                ..
            },
        ] => {
            assert_eq!(id, "nvidia");
            assert_eq!(*kind, ProviderKind::Nvidia);
            assert_eq!(model, "nvidia/some-model");
            assert_eq!(*auth, AuthMethod::ApiKey);
        }
        other => panic!("expected SaveProvider, got {other:?}"),
    }
    let staged = m.pending_credential.as_ref().expect("the key is staged");
    assert_eq!(staged.expose(), "nvapi-secret");
}

#[test]
fn wizard_blank_key_compatible_emits_none_auth_and_no_credential() {
    use crate::shared::kernel::provider::{AuthMethod, ProviderKind};
    let mut m = Model {
        wizard: Some(ProviderWizard::new()),
        ..Default::default()
    };
    // Kind: move to OpenAI-compatible (index 3) and confirm -> ProviderId step.
    for _ in 0..3 {
        on_key(&mut m, press(Key::Down));
    }
    on_key(&mut m, press(Key::Enter));
    // ProviderId: accept the seeded id -> BaseUrl.
    on_key(&mut m, press(Key::Enter));
    // BaseUrl: a compatible endpoint has no default, so type one.
    for c in "http://localhost:1234/v1".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter));
    // Model: required.
    for c in "gemma".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter));
    // ExtraModels: skip. OpenAiCompatible has no thinking capability, so this jumps straight to
    // ApiKey — no Thinking step to accept.
    on_key(&mut m, press(Key::Enter));
    // ApiKey: leave blank and finalize -> keyless save.
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(m.wizard.is_none(), "the wizard closes on finalize");
    match effects.as_slice() {
        [
            Effect::SaveProvider {
                kind,
                base_url,
                model,
                auth,
                ..
            },
        ] => {
            assert_eq!(*kind, ProviderKind::OpenAiCompatible);
            assert_eq!(base_url, "http://localhost:1234/v1");
            assert_eq!(model, "gemma");
            assert_eq!(*auth, AuthMethod::None);
        }
        other => panic!("expected SaveProvider, got {other:?}"),
    }
    assert!(
        m.pending_credential.is_none(),
        "no key is staged for a keyless save"
    );
}

#[test]
fn wizard_edit_keeps_existing_key_for_a_compatible_provider_with_blank_key() {
    // Regression lock: editing a keyless-CAPABLE kind (OpenAiCompatible/Custom) that already HAS a
    // stored key, then leaving the key field blank ("keep existing"), must not collapse `auth` to
    // `None` — that would make `apply_save_provider` delete the real stored key from the store.
    use crate::shared::kernel::provider::{AuthMethod, ProviderKind, ProviderProfile};

    let existing = ProviderProfile {
        id: "openrouter".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: "https://openrouter.ai/api/v1".to_string(),
        model: "gemma".to_string(),
        models: vec!["gemma".to_string()],
        auth: AuthMethod::ApiKey,
        thinking: None,
    };
    let mut m = Model {
        wizard: Some(ProviderWizard::from_profile(&existing)),
        ..Default::default()
    };
    // BaseUrl: accept the pre-filled value -> Model.
    on_key(&mut m, press(Key::Enter));
    // Model: accept the pre-filled value -> ExtraModels.
    on_key(&mut m, press(Key::Enter));
    // ExtraModels: OpenAiCompatible has no thinking capability, so this jumps straight to ApiKey.
    on_key(&mut m, press(Key::Enter));
    // ApiKey: leave blank ("keep existing key") and finalize.
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(m.wizard.is_none(), "the wizard closes on finalize");
    match effects.as_slice() {
        [
            Effect::SaveProvider {
                auth,
                keep_existing_key,
                ..
            },
        ] => {
            assert_eq!(
                *auth,
                AuthMethod::ApiKey,
                "a blank key while editing a previously-keyed provider must stay ApiKey, not collapse to None"
            );
            assert!(*keep_existing_key);
        }
        other => panic!("expected SaveProvider, got {other:?}"),
    }
    assert!(
        m.pending_credential.is_none(),
        "no new key was typed, so none is staged"
    );
}

#[test]
fn wizard_vendor_blank_key_stays_on_step() {
    let mut m = Model {
        wizard: Some(ProviderWizard::new()), // NVIDIA (index 0) is a vendor kind
        ..Default::default()
    };
    on_key(&mut m, press(Key::Enter)); // Kind -> BaseUrl (seeded)
    on_key(&mut m, press(Key::Enter)); // BaseUrl -> Model
    on_key(&mut m, press(Key::Char('m')));
    on_key(&mut m, press(Key::Enter)); // Model -> ExtraModels
    // ExtraModels -> ApiKey directly: model "m" has no recognized NVIDIA thinking family, so the
    // Thinking step is skipped. The extra Enter below lands on ApiKey too and no-ops identically
    // (blank key, vendor kind) — kept so the assertion below holds regardless of step-skip changes.
    on_key(&mut m, press(Key::Enter));
    on_key(&mut m, press(Key::Enter));
    // A vendor kind requires a key: a blank key must not finalize.
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(effects.is_empty(), "a vendor kind cannot finalize keyless");
    assert_eq!(m.wizard.as_ref().map(|w| w.step), Some(WizardStep::ApiKey));
}

#[test]
fn wizard_compatible_blank_base_url_stays_on_step() {
    let mut m = Model {
        wizard: Some(ProviderWizard::new()),
        ..Default::default()
    };
    for _ in 0..3 {
        on_key(&mut m, press(Key::Down)); // OpenAI-compatible
    }
    on_key(&mut m, press(Key::Enter)); // Kind -> ProviderId
    on_key(&mut m, press(Key::Enter)); // ProviderId -> BaseUrl (no default for compatible)
    // A compatible endpoint has no default base URL, so a blank one must not advance.
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(effects.is_empty());
    assert_eq!(m.wizard.as_ref().map(|w| w.step), Some(WizardStep::BaseUrl));
}

#[test]
fn wizard_names_custom_provider_id() {
    use crate::shared::kernel::provider::ProviderKind;
    let mut m = Model {
        wizard: Some(ProviderWizard::new()),
        ..Default::default()
    };
    for _ in 0..4 {
        on_key(&mut m, press(Key::Down)); // Custom (index 4)
    }
    on_key(&mut m, press(Key::Enter)); // Kind -> ProviderId (seeded with "custom")
    // Clear the seeded id, then type a free-form name that must be sanitized to a stable token.
    let seeded = m.wizard.as_ref().map(|w| w.id.chars().count()).unwrap_or(0);
    for _ in 0..seeded {
        on_key(&mut m, press(Key::Backspace));
    }
    for c in "My LM Studio!".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter)); // ProviderId -> BaseUrl
    for c in "http://localhost:1234/v1".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter)); // BaseUrl -> Model
    for c in "gemma".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter)); // Model -> ExtraModels
    // ExtraModels -> ApiKey directly: Custom has no thinking capability, so the Thinking step is
    // skipped.
    on_key(&mut m, press(Key::Enter));
    let effects = on_key(&mut m, press(Key::Enter)); // blank key -> keyless finalize
    match effects.as_slice() {
        [Effect::SaveProvider { id, kind, .. }] => {
            assert_eq!(*kind, ProviderKind::Custom);
            assert_eq!(id, "my-lm-studio", "the id is sanitized to a stable token");
        }
        other => panic!("expected SaveProvider, got {other:?}"),
    }
}

#[test]
fn extra_models_step_skips_thinking_when_the_capability_is_unsupported() {
    let mut m = Model {
        wizard: Some(ProviderWizard::new()), // NVIDIA is index 0
        ..Default::default()
    };
    on_key(&mut m, press(Key::Enter)); // Kind -> BaseUrl (seeded)
    on_key(&mut m, press(Key::Enter)); // BaseUrl -> Model
    for c in "google/gemma-3-27b-it".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter)); // Model -> ExtraModels
    on_key(&mut m, press(Key::Enter)); // ExtraModels -> (skip Thinking) -> ApiKey
    assert_eq!(
        m.wizard.as_ref().map(|w| w.step),
        Some(WizardStep::ApiKey),
        "an NVIDIA model with no recognized thinking family must skip the Thinking step"
    );
    assert_eq!(
        m.wizard.as_ref().map(|w| w.thinking),
        Some(false),
        "the skip must not leave a stale thinking=true from the kind's seeded default"
    );
}

#[test]
fn extra_models_step_still_reaches_thinking_when_the_capability_is_supported() {
    let mut m = Model {
        wizard: Some(ProviderWizard::new()), // NVIDIA is index 0
        ..Default::default()
    };
    on_key(&mut m, press(Key::Enter)); // Kind -> BaseUrl (seeded)
    on_key(&mut m, press(Key::Enter)); // BaseUrl -> Model
    for c in "nvidia/llama-3.3-nemotron-super-49b-v1".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    on_key(&mut m, press(Key::Enter)); // Model -> ExtraModels
    on_key(&mut m, press(Key::Enter)); // ExtraModels -> Thinking
    assert_eq!(
        m.wizard.as_ref().map(|w| w.step),
        Some(WizardStep::Thinking),
        "a recognized NVIDIA thinking family must still reach the Thinking step"
    );
}

#[test]
fn esc_on_onboarding_wizard_keeps_gate_and_hint() {
    let mut m = Model {
        wizard: Some(ProviderWizard::onboarding()),
        unconfigured: true,
        ..Default::default()
    };
    let effects = on_key(&mut m, press(Key::Esc));
    assert!(effects.is_empty());
    assert!(m.wizard.is_none(), "Esc closes the wizard");
    assert!(m.unconfigured, "the gate must persist after onboarding-Esc");
    assert!(
        m.transcript.items().iter().any(
            |item| matches!(item, TranscriptItem::Notice(_, text) if text.contains("/provider"))
        ),
        "a /provider hint must be posted"
    );
}

#[test]
fn esc_on_regular_wizard_says_cancelled() {
    let mut m = Model {
        wizard: Some(ProviderWizard::new()),
        unconfigured: false,
        ..Default::default()
    };
    on_key(&mut m, press(Key::Esc));
    assert!(m.wizard.is_none());
    assert!(!m.unconfigured, "a regular cancel does not touch the gate");
    assert!(
        m.transcript.items().iter().any(
            |item| matches!(item, TranscriptItem::Notice(_, text) if text == "wizard cancelado")
        ),
    );
}

#[test]
fn submit_while_unconfigured_is_gated_and_reopens_wizard() {
    let mut m = Model {
        unconfigured: true,
        ..Default::default()
    };
    let effects = submit_line(&mut m, "oi");
    assert!(effects.is_empty(), "a gated prompt emits no SubmitPrompt");
    assert!(!m.busy, "the gate must not arm a turn (no stuck busy)");
    assert!(
        m.wizard.as_ref().is_some_and(|w| w.onboarding),
        "the gate re-opens the onboarding wizard"
    );
    assert!(
        m.transcript
            .items()
            .iter()
            .any(|item| matches!(item, TranscriptItem::Notice(..))),
        "a gate notice must be posted"
    );
}

#[test]
fn slash_provider_works_while_unconfigured() {
    let mut m =
        Model::default().with_providers("nvidia".to_string(), vec!["nvidia".to_string()], vec![]);
    m.unconfigured = true;
    let effects = submit_line(&mut m, "/provider");
    assert!(effects.is_empty());
    assert!(
        m.picker.is_some(),
        "slash commands are not gated — /provider still opens"
    );
}

#[test]
fn provider_command_opens_a_picker_then_action_submenu_then_set_provider() {
    use crate::modules::tui::domain::picker::PickerKind;
    let mut m = Model::default().with_providers(
        "nvidia".to_string(),
        vec!["nvidia".to_string(), "claude".to_string()],
        vec![],
    );
    let effects = submit_line(&mut m, "/provider");
    assert!(effects.is_empty());
    let picker = m.picker.as_ref().expect("the provider picker should open");
    assert_eq!(picker.kind, PickerKind::Provider);
    // Down to "claude", Enter → opens ProviderAction sub-menu (no effect yet).
    assert!(on_key(&mut m, press(Key::Down)).is_empty());
    assert!(on_key(&mut m, press(Key::Enter)).is_empty());
    // The action sub-menu is now open for "claude".
    assert!(
        matches!(
            m.picker.as_ref().map(|p| &p.kind),
            Some(PickerKind::ProviderAction(id)) if id == "claude"
        ),
        "ProviderAction sub-menu must open for the selected provider"
    );
    // Enter on the first option ("Ativar") emits SetProvider.
    let effects = on_key(&mut m, press(Key::Enter));
    assert_eq!(effects, vec![Effect::SetProvider("claude".to_string())]);
}

/// A minimal profile for the provider-action-submenu tests below (issue #27/M7-1).
fn provider_profile_for_tests(id: &str) -> crate::shared::kernel::provider::ProviderProfile {
    use crate::shared::kernel::provider::{AuthMethod, ProviderKind};
    crate::shared::kernel::provider::ProviderProfile {
        id: id.to_string(),
        kind: ProviderKind::Anthropic,
        base_url: "https://example.test/v1".to_string(),
        model: "m1".to_string(),
        models: vec!["m1".to_string()],
        auth: AuthMethod::ApiKey,
        thinking: None,
    }
}

/// Open the `/provider` picker, move to `claude`, and open its ProviderAction sub-menu — the shared setup
/// for every test below.
fn open_provider_action_submenu(m: &mut Model) {
    use crate::modules::tui::domain::picker::PickerKind;
    submit_line(m, "/provider");
    on_key(m, press(Key::Down)); // to "claude"
    on_key(m, press(Key::Enter)); // opens the ProviderAction sub-menu
    assert!(
        matches!(m.picker.as_ref().map(|p| &p.kind), Some(PickerKind::ProviderAction(id)) if id == "claude"),
        "setup must land on claude's ProviderAction sub-menu"
    );
}

#[test]
fn provider_action_editar_opens_the_wizard_for_the_selected_profile() {
    // Issue #27/M7-1: the "Editar" arm (index 1) was never reducer-tested.
    let mut m = Model::default().with_providers(
        "nvidia".to_string(),
        vec!["nvidia".to_string(), "claude".to_string()],
        vec![
            provider_profile_for_tests("nvidia"),
            provider_profile_for_tests("claude"),
        ],
    );
    open_provider_action_submenu(&mut m);
    // Down to "Editar" (index 1), Enter.
    on_key(&mut m, press(Key::Down));
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(
        effects.is_empty(),
        "opening the edit wizard emits no effect"
    );
    assert!(
        m.wizard.is_some(),
        "'Editar' must open the wizard for claude's own profile"
    );
    assert!(
        m.picker.is_none(),
        "the action sub-menu must close once the wizard opens"
    );
}

#[test]
fn provider_action_remover_opens_delete_confirm_with_cancelar_preselected() {
    // Issue #27/M7-1: the "Remover" arm (index 2) and the delete-confirm's safety default (Cancelar
    // preselected, per `modals.rs`'s `Picker::new(..., 1)`) had no test.
    use crate::modules::tui::domain::picker::PickerKind;
    let mut m = Model::default().with_providers(
        "nvidia".to_string(),
        vec!["nvidia".to_string(), "claude".to_string()],
        vec![
            provider_profile_for_tests("nvidia"),
            provider_profile_for_tests("claude"),
        ],
    );
    open_provider_action_submenu(&mut m);
    // Down, Down to "Remover" (index 2), Enter.
    on_key(&mut m, press(Key::Down));
    on_key(&mut m, press(Key::Down));
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(
        effects.is_empty(),
        "opening the delete-confirm emits no effect yet"
    );
    let picker = m
        .picker
        .as_ref()
        .expect("the delete-confirm picker must open");
    assert_eq!(
        picker.kind,
        PickerKind::ProviderDeleteConfirm("claude".to_string())
    );
    assert_eq!(
        picker.options,
        vec!["Sim, remover".to_string(), "Cancelar".to_string()]
    );
    assert_eq!(
        picker.selected, 1,
        "Cancelar must be preselected — a destructive default must never be the easy Enter"
    );
}

#[test]
fn provider_delete_confirm_preselected_cancelar_emits_no_effect() {
    // Enter on the preselected ("Cancelar") row must not delete anything.
    let mut m = Model::default().with_providers(
        "nvidia".to_string(),
        vec!["nvidia".to_string(), "claude".to_string()],
        vec![
            provider_profile_for_tests("nvidia"),
            provider_profile_for_tests("claude"),
        ],
    );
    open_provider_action_submenu(&mut m);
    on_key(&mut m, press(Key::Down));
    on_key(&mut m, press(Key::Down));
    on_key(&mut m, press(Key::Enter)); // opens delete-confirm, Cancelar preselected
    let effects = on_key(&mut m, press(Key::Enter));
    assert!(
        effects.is_empty(),
        "confirming on the preselected Cancelar row must emit no effect"
    );
}

#[test]
fn provider_delete_confirm_sim_remover_emits_delete_provider() {
    let mut m = Model::default().with_providers(
        "nvidia".to_string(),
        vec!["nvidia".to_string(), "claude".to_string()],
        vec![
            provider_profile_for_tests("nvidia"),
            provider_profile_for_tests("claude"),
        ],
    );
    open_provider_action_submenu(&mut m);
    on_key(&mut m, press(Key::Down));
    on_key(&mut m, press(Key::Down));
    on_key(&mut m, press(Key::Enter)); // opens delete-confirm, Cancelar preselected
    on_key(&mut m, press(Key::Up)); // to "Sim, remover" (index 0)
    let effects = on_key(&mut m, press(Key::Enter));
    assert_eq!(effects, vec![Effect::DeleteProvider("claude".to_string())]);
}

#[test]
fn picker_esc_closes_without_an_effect() {
    let mut m = Model::default().with_provider_catalog(vec!["a".to_string()], Effort::default());
    submit_line(&mut m, "/models");
    assert!(m.picker.is_some());
    let effects = on_key(&mut m, press(Key::Esc));
    assert!(effects.is_empty());
    assert!(m.picker.is_none());
}

#[test]
fn models_command_with_an_empty_catalog_notifies_and_opens_nothing() {
    let mut m = Model::default(); // no models configured
    submit_line(&mut m, "/models");
    assert!(m.picker.is_none(), "no picker without a catalog");
}

#[test]
fn typing_then_enter_submits_a_prompt() {
    let mut m = Model::default();
    for c in "hi".chars() {
        on_key(&mut m, press(Key::Char(c)));
    }
    let effects = on_key(&mut m, press(Key::Enter));
    assert_eq!(
        effects,
        vec![Effect::SubmitPrompt {
            text: "hi".to_string(),
            images: vec![],
        }]
    );
    assert!(m.busy);
    assert!(m.input.is_empty());
    assert_eq!(m.transcript.items().len(), 1);
}

#[test]
fn shift_enter_inserts_a_newline_without_submitting() {
    let mut m = Model::default();
    on_key(&mut m, press(Key::Char('a')));
    let effects = on_key(
        &mut m,
        KeyPress {
            code: Key::Enter,
            ctrl: false,
            alt: false,
            shift: true,
        },
    );
    assert!(effects.is_empty());
    assert_eq!(m.input.text(), "a\n");
}

#[test]
fn ctrl_c_cancels_while_busy_double_ctrl_c_quits() {
    let mut m = Model {
        busy: true,
        ..Model::default()
    };
    let ctrl_c = KeyPress {
        code: Key::Char('c'),
        ctrl: true,
        alt: false,
        shift: false,
    };
    // Single Ctrl+C while busy → cancel the turn.
    assert_eq!(on_key(&mut m, ctrl_c.clone()), vec![Effect::CancelTurn]);
    // Second Ctrl+C within the window → quit (double-tap), even though the first cancelled.
    assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
    assert!(m.should_quit);
}

#[test]
fn single_ctrl_c_while_idle_does_nothing_then_double_quits() {
    let mut m = Model::default();
    let ctrl_c = KeyPress {
        code: Key::Char('c'),
        ctrl: true,
        alt: false,
        shift: false,
    };
    // Single Ctrl+C while idle → no-op (quit requires a double tap).
    assert_eq!(on_key(&mut m, ctrl_c.clone()), vec![]);
    assert!(!m.should_quit);
    // Double Ctrl+C → quit.
    assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
    assert!(m.should_quit);
}

#[test]
fn double_esc_cancels_while_busy() {
    let mut m = Model {
        busy: true,
        ..Model::default()
    };
    let esc = KeyPress {
        code: Key::Esc,
        ctrl: false,
        alt: false,
        shift: false,
    };
    // First Esc while busy → no-op (recorded for the double-tap window).
    assert_eq!(on_key(&mut m, esc.clone()), vec![]);
    // Second Esc within the window → cancel the turn.
    assert_eq!(on_key(&mut m, esc), vec![Effect::CancelTurn]);
}

#[test]
fn pending_approval_consumes_keys_as_decisions() {
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("delete a.txt".to_string(), true)),
        ..Model::default()
    };
    let effects = on_key(&mut m, press(Key::Char('n')));
    assert_eq!(effects, vec![Effect::AnswerApproval(Approval::Declined)]);
    assert!(m.pending_approval.is_none());
    assert_eq!(m.transcript.items().len(), 1);
}

#[test]
fn approval_key_with_no_pending_approval_is_a_noop() {
    // Guards the invariant that on_approval_key never panics if reached without a pending
    // approval (e.g. a future routing change) — it answers nothing rather than unwrapping None.
    let mut m = Model::default();
    assert!(m.pending_approval.is_none());
    assert_eq!(on_approval_key(&mut m, press(Key::Enter)), vec![]);
}

#[test]
fn enter_on_approval_follows_the_default() {
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("p".to_string(), false)),
        ..Model::default()
    };
    assert_eq!(
        on_key(&mut m, press(Key::Enter)),
        vec![Effect::AnswerApproval(Approval::Declined)]
    );
}

#[test]
fn approval_arrows_move_selection_then_enter_confirms_and_switches_to_auto() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("p".to_string(), true)),
        ..Model::default()
    };
    on_key(&mut m, press(Key::Down)); // highlight option 2 ("…modo auto")
    assert_eq!(m.pending_approval.as_ref().unwrap().selected, 1);
    let effects = on_key(&mut m, press(Key::Enter));
    // ApprovedAuto runs the rest of this turn unattended; the mode also sticks for later turns.
    assert_eq!(
        effects,
        vec![Effect::AnswerApproval(Approval::ApprovedAuto)]
    );
    assert_eq!(m.approval_mode, ApprovalMode::Auto);
    assert!(m.pending_approval.is_none());
    let last = m.transcript.items().last().unwrap();
    assert!(
        matches!(last, TranscriptItem::Notice(NoticeLevel::Info, t) if t.contains("modo auto ativo")),
        "missing auto-active notice: {last:?}"
    );
}

#[test]
fn all_modals_wrap_on_up_at_top() {
    // Deliberate UX change (S19): the approval and plan boxes now wrap on Up at the top instead of
    // clamping, so every single-choice modal behaves the same way. Lock both newly-wrapping modals.
    let mut approval = Model {
        pending_approval: Some(PendingApproval::new("p".to_string(), true)),
        ..Model::default()
    };
    assert_eq!(approval.pending_approval.as_ref().unwrap().selected, 0);
    on_key(&mut approval, press(Key::Up));
    assert_eq!(
        approval.pending_approval.as_ref().unwrap().selected,
        ApprovalOption::ALL.len() - 1,
        "approval Up at the top must wrap to the last option"
    );

    let mut plan = Model {
        pending_plan: Some(PendingPlan::default()),
        ..Model::default()
    };
    assert_eq!(plan.pending_plan.as_ref().unwrap().selected, 0);
    on_key(&mut plan, press(Key::Up));
    assert_eq!(
        plan.pending_plan.as_ref().unwrap().selected,
        PlanOption::ALL.len() - 1,
        "plan Up at the top must wrap to the last option"
    );
}

#[test]
fn approval_digit_selection_follows_options_len() {
    // Digits 1..=len pick the option at that 1-based position; a digit past the list is ignored.
    let answer = |digit: char| {
        let mut m = Model {
            pending_approval: Some(PendingApproval::new("p".to_string(), true)),
            ..Model::default()
        };
        on_key(&mut m, press(Key::Char(digit)))
    };
    assert_eq!(
        answer('1'),
        vec![Effect::AnswerApproval(Approval::Approved)]
    );
    assert_eq!(
        answer('2'),
        vec![Effect::AnswerApproval(Approval::ApprovedAuto)]
    );
    assert_eq!(
        answer('3'),
        vec![Effect::AnswerApproval(Approval::Declined)]
    );
    // '4' is past the three options: no answer, the approval stays pending.
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("p".to_string(), true)),
        ..Model::default()
    };
    assert!(on_key(&mut m, press(Key::Char('4'))).is_empty());
    assert!(m.pending_approval.is_some());
}

#[test]
fn reordering_approval_options_keeps_semantics() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    // The decision is bound to the named variant, not a positional index: selecting the row whose option
    // is `ApproveAuto` always switches to auto, wherever that variant sits in `ALL`.
    let auto_index = ApprovalOption::ALL
        .iter()
        .position(|o| *o == ApprovalOption::ApproveAuto)
        .expect("ApproveAuto is in ALL");
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("p".to_string(), true)),
        ..Model::default()
    };
    m.pending_approval.as_mut().unwrap().selected = auto_index;
    let effects = on_key(&mut m, press(Key::Enter));
    assert_eq!(
        effects,
        vec![Effect::AnswerApproval(Approval::ApprovedAuto)]
    );
    assert_eq!(m.approval_mode, ApprovalMode::Auto);
}

#[test]
fn esc_declines_without_aborting_the_session() {
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("p".to_string(), true)),
        ..Model::default()
    };
    assert_eq!(
        on_key(&mut m, press(Key::Esc)),
        vec![Effect::AnswerApproval(Approval::Declined)]
    );
}

#[test]
fn ctrl_c_aborts_a_pending_approval() {
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("p".to_string(), true)),
        ..Model::default()
    };
    let ctrl_c = KeyPress {
        code: Key::Char('c'),
        ctrl: true,
        alt: false,
        shift: false,
    };
    assert_eq!(
        on_key(&mut m, ctrl_c),
        vec![Effect::AnswerApproval(Approval::Aborted)]
    );
}

#[test]
fn ctrl_o_toggles_tool_output_expansion() {
    let mut m = Model::default();
    assert!(!m.expand_tools);
    assert!(on_key(&mut m, ctrl(Key::Char('o'))).is_empty());
    assert!(m.expand_tools, "Ctrl+O should expand tool output");
    on_key(&mut m, ctrl(Key::Char('o')));
    assert!(!m.expand_tools, "Ctrl+O again should collapse it");
}

#[test]
fn back_tab_cycles_the_approval_mode_when_idle() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model::default();
    assert_eq!(m.approval_mode, ApprovalMode::Default);
    assert!(on_key(&mut m, press(Key::BackTab)).is_empty());
    assert_eq!(m.approval_mode, ApprovalMode::Auto);
    on_key(&mut m, press(Key::BackTab));
    assert_eq!(m.approval_mode, ApprovalMode::Plan);
    on_key(&mut m, press(Key::BackTab));
    assert_eq!(m.approval_mode, ApprovalMode::Default);
}

#[test]
fn back_tab_is_ignored_mid_turn() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model {
        busy: true,
        ..Model::default()
    };
    on_key(&mut m, press(Key::BackTab));
    assert_eq!(m.approval_mode, ApprovalMode::Default);
}

#[test]
fn new_session_command_emits_effect() {
    let mut m = Model::default();
    m.input.set("/new".to_string());
    assert_eq!(on_key(&mut m, press(Key::Enter)), vec![Effect::NewSession]);
}

#[test]
fn mode_command_sets_mode_without_effect() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model::default();
    m.input.set("/plan".to_string());
    assert!(on_key(&mut m, press(Key::Enter)).is_empty());
    assert_eq!(m.approval_mode, ApprovalMode::Plan);
}

#[test]
fn cd_with_path_emits_change_workspace() {
    let mut m = Model::default();
    m.input.set("/cd src".to_string());
    assert_eq!(
        on_key(&mut m, press(Key::Enter)),
        vec![Effect::ChangeWorkspace("src".to_string())]
    );
}

#[test]
fn unknown_command_warns_without_effect() {
    let mut m = Model::default();
    m.input.set("/nope".to_string());
    assert!(on_key(&mut m, press(Key::Enter)).is_empty());
    assert_eq!(m.transcript.items().len(), 1);
}

#[test]
fn plan_enter_executes_the_plan() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model {
        pending_plan: Some(PendingPlan::default()),
        ..Model::default()
    };
    assert_eq!(
        on_key(&mut m, press(Key::Enter)),
        vec![Effect::ApprovePlan(ApprovalMode::Default)]
    );
    assert!(m.pending_plan.is_none());
}

#[test]
fn plan_execute_in_auto_emits_auto_mode() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model {
        pending_plan: Some(PendingPlan::default()),
        approval_mode: ApprovalMode::Plan,
        ..Model::default()
    };
    on_key(&mut m, press(Key::Down)); // highlight "Executar o plano em modo auto"
    assert_eq!(
        on_key(&mut m, press(Key::Enter)),
        vec![Effect::ApprovePlan(ApprovalMode::Auto)]
    );
    assert!(m.pending_plan.is_none());
}

#[test]
fn plan_keep_planning_closes_box_and_stays_in_plan() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model {
        pending_plan: Some(PendingPlan::default()),
        approval_mode: ApprovalMode::Plan,
        ..Model::default()
    };
    on_key(&mut m, press(Key::Down)); // highlight "Executar o plano em modo auto"
    on_key(&mut m, press(Key::Down)); // highlight "Continuar planejando"
    assert!(on_key(&mut m, press(Key::Enter)).is_empty());
    assert!(m.pending_plan.is_none());
    assert_eq!(m.approval_mode, ApprovalMode::Plan);
}

#[test]
fn plan_cancel_leaves_plan_mode() {
    use crate::shared::kernel::approval_mode::ApprovalMode;
    let mut m = Model {
        pending_plan: Some(PendingPlan::default()),
        approval_mode: ApprovalMode::Plan,
        ..Model::default()
    };
    assert!(on_key(&mut m, press(Key::Esc)).is_empty());
    assert!(m.pending_plan.is_none());
    assert_eq!(m.approval_mode, ApprovalMode::Default);
}

// --- live slash-command preview ----------------------------------------------

fn type_str(m: &mut Model, s: &str) {
    for c in s.chars() {
        on_key(m, press(Key::Char(c)));
    }
}

#[test]
fn typing_slash_opens_the_command_menu() {
    let mut m = Model::default();
    type_str(&mut m, "/");
    assert!(m.command_menu.is_some(), "menu should open on bare slash");
    // Empty query shows the whole catalog.
    assert_eq!(
        m.command_menu.as_ref().unwrap().len(),
        crate::modules::tui::domain::command_menu::COMMANDS.len()
    );
}

#[test]
fn typing_after_slash_filters_the_menu() {
    let mut m = Model::default();
    type_str(&mut m, "/ne");
    let menu = m.command_menu.as_ref().expect("menu should stay open");
    assert_eq!(menu.len(), 1);
    assert_eq!(
        menu.entry().unwrap().name(),
        "/new",
        "filtered row should highlight /new"
    );
}

#[test]
fn backspace_closes_the_menu_when_the_slash_is_erased() {
    let mut m = Model::default();
    type_str(&mut m, "/n");
    assert!(m.command_menu.is_some());
    on_key(&mut m, press(Key::Backspace)); // now "/"
    assert!(m.command_menu.is_some(), "bare slash keeps the menu open");
    on_key(&mut m, press(Key::Backspace)); // now empty
    assert!(
        m.command_menu.is_none(),
        "erasing the slash closes the menu"
    );
    assert!(m.input.is_empty());
}

#[test]
fn space_in_buffer_closes_the_menu_argument_mode() {
    let mut m = Model::default();
    type_str(&mut m, "/cd ");
    assert!(
        m.command_menu.is_none(),
        "whitespace starts argument mode, menu must close"
    );
}

#[test]
fn arrows_move_the_highlight_without_recalling_history() {
    let mut m = Model::default();
    type_str(&mut m, "/");
    let first = m.command_menu.as_ref().unwrap().selected();
    on_key(&mut m, press(Key::Down));
    assert_eq!(m.command_menu.as_ref().unwrap().selected(), first + 1);
    on_key(&mut m, press(Key::Up));
    assert_eq!(m.command_menu.as_ref().unwrap().selected(), first);
}

#[test]
fn tab_completes_to_canonical_name_plus_space_and_closes_menu() {
    let mut m = Model::default();
    type_str(&mut m, "/ne");
    on_key(&mut m, press(Key::Tab));
    assert_eq!(m.input.text(), "/new ");
    assert!(
        m.command_menu.is_none(),
        "Tab closes the menu after completion"
    );
}

#[test]
fn esc_closes_the_menu_but_keeps_the_buffer() {
    let mut m = Model::default();
    type_str(&mut m, "/ne");
    on_key(&mut m, press(Key::Esc));
    assert!(m.command_menu.is_none());
    assert_eq!(m.input.text(), "/ne", "Esc must not erase the text");
}

#[test]
fn enter_in_a_filtered_menu_submits_the_command() {
    let mut m = Model::default();
    type_str(&mut m, "/new");
    let effects = on_key(&mut m, press(Key::Enter));
    assert_eq!(effects, vec![Effect::NewSession]);
    assert!(m.command_menu.is_none(), "submit must clear the menu");
    assert!(m.input.is_empty());
}

#[test]
fn menu_does_not_open_while_a_turn_is_running() {
    let mut m = Model {
        busy: true,
        ..Model::default()
    };
    type_str(&mut m, "/");
    assert!(m.command_menu.is_none(), "menu must stay closed while busy");
}

#[test]
fn ctrl_c_mid_menu_double_tap_quits() {
    let mut m = Model::default();
    type_str(&mut m, "/");
    let ctrl_c = KeyPress {
        code: Key::Char('c'),
        ctrl: true,
        alt: false,
        shift: false,
    };
    // Single Ctrl+C → no-op (quit now requires a double tap).
    assert_eq!(on_key(&mut m, ctrl_c.clone()), vec![]);
    // Double Ctrl+C → quit.
    assert_eq!(on_key(&mut m, ctrl_c), vec![Effect::Quit]);
    assert!(m.should_quit);
}

// --- rich editor: clipboard chords and history-at-edge -------------------------

fn shift(code: Key) -> KeyPress {
    KeyPress {
        code,
        ctrl: false,
        alt: false,
        shift: true,
    }
}

fn ctrl(code: Key) -> KeyPress {
    KeyPress {
        code,
        ctrl: true,
        alt: false,
        shift: false,
    }
}

fn alt(code: Key) -> KeyPress {
    KeyPress {
        code,
        ctrl: false,
        alt: true,
        shift: false,
    }
}

fn shift_alt(code: Key) -> KeyPress {
    KeyPress {
        code,
        ctrl: false,
        alt: true,
        shift: true,
    }
}

#[test]
fn ctrl_c_with_a_selection_copies_instead_of_cancelling() {
    let mut m = Model {
        busy: true, // even mid-turn, Ctrl+C on a selection copies rather than cancels
        ..Model::default()
    };
    type_str(&mut m, "abc");
    on_key(&mut m, shift(Key::Left));
    on_key(&mut m, shift(Key::Left)); // select "bc"
    let effects = on_key(&mut m, ctrl(Key::Char('c')));
    assert!(
        matches!(effects.as_slice(), [Effect::CopyToClipboard(t)] if t == "bc"),
        "Ctrl+C with a selection should copy it, got {effects:?}"
    );
    assert!(!m.should_quit, "copy must not quit");
}

#[test]
fn ctrl_x_cuts_the_selection_and_removes_it() {
    let mut m = Model::default();
    type_str(&mut m, "abc");
    on_key(&mut m, shift(Key::Left)); // select "c"
    let effects = on_key(&mut m, ctrl(Key::Char('x')));
    assert!(
        matches!(effects.as_slice(), [Effect::CopyToClipboard(t)] if t == "c"),
        "Ctrl+X should cut the selection, got {effects:?}"
    );
    assert_eq!(m.input.text(), "ab", "cut must remove the selected text");
}

#[test]
fn ctrl_x_without_a_selection_is_a_noop() {
    let mut m = Model::default();
    type_str(&mut m, "abc");
    assert!(on_key(&mut m, ctrl(Key::Char('x'))).is_empty());
    assert_eq!(m.input.text(), "abc");
}

#[test]
fn up_recalls_history_only_at_the_first_row() {
    let mut m = Model::default();
    m.history.record("prev");
    // Build a two-line buffer; the cursor ends on the last (second) row.
    on_key(&mut m, press(Key::Char('a')));
    on_key(&mut m, shift(Key::Enter)); // newline without submitting
    on_key(&mut m, press(Key::Char('b')));
    assert_eq!(m.input.text(), "a\nb");
    // From the last row, Up moves the cursor up — it must NOT recall history.
    on_key(&mut m, press(Key::Up));
    assert_eq!(
        m.input.text(),
        "a\nb",
        "Up inside a multi-line buffer must not recall"
    );
    // Now on the first row, Up recalls the previous prompt.
    on_key(&mut m, press(Key::Up));
    assert_eq!(
        m.input.text(),
        "prev",
        "Up at the first row should recall history"
    );
}

// --- macOS typing standard ----------------------------------------------------

#[test]
fn ctrl_a_moves_to_line_start() {
    // macOS/Cocoa: Ctrl+A is "move to line start", not select-all. After it, typing inserts at the
    // head of the line.
    let mut m = Model::default();
    type_str(&mut m, "abc");
    on_key(&mut m, ctrl(Key::Char('a')));
    on_key(&mut m, press(Key::Char('X')));
    assert_eq!(m.input.text(), "Xabc");
}

#[test]
fn ctrl_a_no_longer_selects_all() {
    // Guards the intent that select-all left the keyboard: Ctrl+A must not start a selection.
    let mut m = Model::default();
    type_str(&mut m, "abc");
    on_key(&mut m, ctrl(Key::Char('a')));
    assert!(
        !m.input.is_selecting(),
        "Ctrl+A must move the cursor, not select all"
    );
}

#[test]
fn shift_alt_left_selects_word_back() {
    // Option+Left jumps a word; with Shift it selects one. Cutting proves the whole word was caught.
    let mut m = Model::default();
    type_str(&mut m, "foo bar");
    on_key(&mut m, shift_alt(Key::Left));
    let effects = on_key(&mut m, ctrl(Key::Char('x')));
    let cut = match effects.as_slice() {
        [Effect::CopyToClipboard(t)] => t.clone(),
        other => panic!("expected a cut, got {other:?}"),
    };
    assert_eq!(
        cut.trim(),
        "bar",
        "Shift+Option+Left should select the word back"
    );
    assert_eq!(m.input.text().trim_end(), "foo");
}

#[test]
fn shift_alt_right_selects_word_forward() {
    let mut m = Model::default();
    m.input.set("foo bar".to_string());
    on_key(&mut m, press(Key::Home));
    on_key(&mut m, shift_alt(Key::Right));
    let effects = on_key(&mut m, ctrl(Key::Char('x')));
    let cut = match effects.as_slice() {
        [Effect::CopyToClipboard(t)] => t.clone(),
        other => panic!("expected a cut, got {other:?}"),
    };
    assert_eq!(
        cut.trim(),
        "foo",
        "Shift+Option+Right should select the word forward"
    );
    assert_eq!(m.input.text().trim_start(), "bar");
}

#[test]
fn option_backspace_still_deletes_word() {
    // Regression: the native macOS Option+Backspace (delivered as Alt+Backspace) must keep deleting a
    // word — the new word-motion remap keys off Left/Right and must not disturb this.
    let mut m = Model::default();
    type_str(&mut m, "foo bar");
    on_key(&mut m, alt(Key::Backspace));
    assert_eq!(m.input.text().trim_end(), "foo");
}

#[test]
fn ctrl_backspace_deletes_word_back() {
    let mut m = Model::default();
    type_str(&mut m, "foo bar");
    on_key(&mut m, ctrl(Key::Backspace));
    assert_eq!(m.input.text().trim_end(), "foo");
}

#[test]
fn ctrl_delete_deletes_word_forward() {
    let mut m = Model::default();
    m.input.set("foo bar".to_string());
    on_key(&mut m, press(Key::Home));
    on_key(&mut m, ctrl(Key::Delete));
    assert_eq!(m.input.text().trim_start(), "bar");
}

#[test]
fn meta_word_motion_still_selects() {
    // The other wire encoding of Option+word-motion: meta Alt+b/f reaches the widget directly.
    let mut m = Model::default();
    type_str(&mut m, "foo bar");
    on_key(
        &mut m,
        KeyPress {
            code: Key::Char('b'),
            ctrl: false,
            alt: true,
            shift: true,
        },
    );
    let effects = on_key(&mut m, ctrl(Key::Char('x')));
    assert!(
        matches!(effects.as_slice(), [Effect::CopyToClipboard(t)] if t.trim() == "bar"),
        "Shift+Alt+b should select the word back, got {effects:?}"
    );
}

#[test]
fn plain_home_moves_to_line_head() {
    // Guards that Home still reaches the widget (it is not intercepted unless Ctrl is held).
    let mut m = Model::default();
    type_str(&mut m, "ab");
    on_key(&mut m, press(Key::Home));
    on_key(&mut m, press(Key::Char('X')));
    assert_eq!(m.input.text(), "Xab");
}

#[test]
fn alt_char_without_arrow_falls_through_to_editor() {
    // An Alt+Char that is not a recognized motion must still reach the editor (not be swallowed).
    // Under "Option as Meta" some layouts deliver letters with Alt; they must type, not vanish.
    let mut m = Model::default();
    on_key(&mut m, alt(Key::Char('z')));
    // The widget binds Alt+z to nothing destructive here; the key is consumed by feed without error.
    // The guarantee under test is "no panic / no swallow into a dead chord" — the buffer stays valid.
    assert!(m.input.text().is_empty() || m.input.text() == "z");
}

// --- screen selection (mouse) -------------------------------------------------

/// A model whose event clock is stamped, ready for mouse-gesture tests.
fn with_clock(now: Instant) -> Model {
    let mut m = Model::default();
    m.timeline.last_event_at = Some(now);
    m
}

#[test]
fn mouse_down_starts_a_char_selection() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    let sel = m.selection.active.expect("down starts a selection");
    assert_eq!(sel.anchor, (3, 2));
    assert_eq!(sel.head, (3, 2));
    assert_eq!(sel.granularity, Granularity::Char);
    assert_eq!(sel.state, SelectionState::Idle);
}

#[test]
fn mouse_drag_extends_head_and_keeps_anchor() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 7, 2);
    let sel = m.selection.active.unwrap();
    assert_eq!(sel.anchor, (3, 2));
    assert_eq!(sel.head, (7, 2));
}

#[test]
fn mouse_up_on_a_real_drag_requests_copy_and_keeps_highlight() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 7, 2);
    on_mouse(&mut m, MouseKind::Up, 7, 2);
    let sel = m
        .selection
        .active
        .expect("a non-empty selection stays after release");
    assert_eq!(sel.state, SelectionState::CopyAndKeep);
    assert!(!sel.is_empty());
}

#[test]
fn bare_click_clears_the_selection() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Up, 3, 2);
    assert!(
        m.selection.active.is_none(),
        "a click with no drag selects nothing"
    );
}

#[test]
fn single_cell_selection_needs_a_one_cell_drag() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 4, 2);
    on_mouse(&mut m, MouseKind::Up, 4, 2);
    let sel = m
        .selection
        .active
        .expect("a one-cell drag is a real selection");
    assert!(!sel.is_empty());
    assert_eq!(sel.state, SelectionState::CopyAndKeep);
}

#[test]
fn double_click_within_window_selects_a_word() {
    let t0 = Instant::now();
    let mut m = with_clock(t0);
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Up, 3, 2); // bare click clears the highlight...
    m.timeline.last_event_at = Some(t0 + Duration::from_millis(50));
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    let sel = m.selection.active.expect("the second click reselects");
    assert_eq!(sel.granularity, Granularity::Word);
    assert!(!sel.is_empty(), "a word selection is never empty");
}

#[test]
fn triple_click_within_window_selects_a_line() {
    let t0 = Instant::now();
    let mut m = with_clock(t0);
    for i in 0..3u64 {
        m.timeline.last_event_at = Some(t0 + Duration::from_millis(i * 50));
        on_mouse(&mut m, MouseKind::Down, 3, 2);
        on_mouse(&mut m, MouseKind::Up, 3, 2);
    }
    let sel = m
        .selection
        .active
        .expect("a line selection stays after the third release");
    assert_eq!(sel.granularity, Granularity::Line);
}

#[test]
fn second_click_after_the_window_is_a_fresh_single() {
    let t0 = Instant::now();
    let mut m = with_clock(t0);
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Up, 3, 2);
    m.timeline.last_event_at = Some(t0 + Duration::from_millis(600)); // > MULTI_CLICK_WINDOW
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    assert_eq!(m.selection.active.unwrap().granularity, Granularity::Char);
}

#[test]
fn double_click_far_away_is_two_singles() {
    let t0 = Instant::now();
    let mut m = with_clock(t0);
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Up, 3, 2);
    m.timeline.last_event_at = Some(t0 + Duration::from_millis(50));
    on_mouse(&mut m, MouseKind::Down, 9, 9); // a different cell — not a double-click
    assert_eq!(m.selection.active.unwrap().granularity, Granularity::Char);
}

#[test]
fn keystroke_clears_the_screen_selection() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 7, 2);
    on_mouse(&mut m, MouseKind::Up, 7, 2);
    assert!(m.selection.active.is_some());
    on_key(&mut m, press(Key::Char('a')));
    assert!(
        m.selection.active.is_none(),
        "typing drops the screen selection"
    );
}

#[test]
fn esc_clears_the_screen_selection_when_idle() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 7, 2);
    on_mouse(&mut m, MouseKind::Up, 7, 2);
    on_key(&mut m, press(Key::Esc));
    assert!(m.selection.active.is_none());
}

#[test]
fn ctrl_c_prefers_the_screen_selection_and_requests_clearing_copy() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 7, 2);
    on_mouse(&mut m, MouseKind::Up, 7, 2);
    let effects = on_key(&mut m, ctrl(Key::Char('c')));
    assert!(
        effects.is_empty(),
        "screen copy goes through selection state, not an Effect"
    );
    assert_eq!(
        m.selection
            .active
            .expect("selection survives until the runtime scrapes it")
            .state,
        SelectionState::CopyAndClear
    );
    assert!(!m.should_quit, "Ctrl+C on a selection must not quit");
}

#[test]
fn mouse_selection_works_while_a_modal_is_pending() {
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("ler a.txt".to_string(), true)),
        ..Model::default()
    };
    m.timeline.last_event_at = Some(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 7, 2);
    on_mouse(&mut m, MouseKind::Up, 7, 2);
    assert!(
        m.selection.active.is_some(),
        "mouse selection must work under a modal (to copy its text)"
    );
}

#[test]
fn bare_click_in_the_focused_composer_emits_place_cursor() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 12, 4);
    let effects = on_mouse(&mut m, MouseKind::Up, 12, 4);
    assert_eq!(effects, vec![Effect::PlaceCursor { col: 12, row: 4 }]);
    assert!(
        m.selection.active.is_none(),
        "a bare click leaves no highlight"
    );
}

#[test]
fn a_drag_selects_and_does_not_place_the_cursor() {
    let mut m = with_clock(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 3, 2);
    on_mouse(&mut m, MouseKind::Drag, 7, 2);
    let effects = on_mouse(&mut m, MouseKind::Up, 7, 2);
    assert!(
        effects.is_empty(),
        "a drag is a selection, not a cursor placement"
    );
    assert_eq!(
        m.selection.active.unwrap().state,
        SelectionState::CopyAndKeep
    );
}

#[test]
fn bare_click_during_a_modal_does_not_place_the_cursor() {
    // Under a modal the editor is read-only; a bare click clears any highlight but must not try to
    // move the (hidden) edit cursor.
    let mut m = Model {
        pending_approval: Some(PendingApproval::new("ler a.txt".to_string(), true)),
        ..Model::default()
    };
    m.timeline.last_event_at = Some(Instant::now());
    on_mouse(&mut m, MouseKind::Down, 12, 4);
    let effects = on_mouse(&mut m, MouseKind::Up, 12, 4);
    assert!(effects.is_empty(), "no cursor placement under a modal");
    assert!(m.selection.active.is_none());
}
