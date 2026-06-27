use super::*;

/// Drive the add-provider wizard. The `Kind` step uses arrows + Enter; the text steps take typed
/// characters + Backspace, advance on Enter, and the final `ApiKey` step finalizes — staging the key in
/// `Model::pending_credential` (a `Secret`) and emitting `SaveProvider` (no secret). Esc cancels any
/// step, Ctrl+C quits.
pub(super) fn on_wizard_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if key.ctrl && key.code == Key::Char('c') {
        model.wizard = None;
        model.should_quit = true;
        return vec![Effect::Quit];
    }
    if key.code == Key::Esc {
        // Cancelling onboarding must not strand a credential-less app: keep the submit gate up and post a
        // persistent hint. The next stray prompt re-opens onboarding (via the gate), and /provider works.
        let onboarding = model.wizard.as_ref().is_some_and(|w| w.onboarding);
        model.wizard = None;
        let message = if onboarding {
            "configure um provider com /provider para começar"
        } else {
            "wizard cancelado"
        };
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            message.to_string(),
        ));
        return vec![];
    }

    let Some(wizard) = model.wizard.as_mut() else {
        return vec![];
    };

    // The Kind step is a chooser; the rest are text fields.
    if wizard.step == WizardStep::Kind {
        match key.code {
            Key::Up => wizard.move_kind(-1),
            Key::Down => wizard.move_kind(1),
            Key::Enter => {
                if wizard.key_required() {
                    // Vendor kinds use a canonical id; go straight to the base URL, seeded with the
                    // kind's default so the common case is one keystroke (Enter).
                    wizard.step = WizardStep::BaseUrl;
                    if wizard.base_url.is_empty() {
                        wizard.base_url = wizard.kind().default_base_url().to_string();
                    }
                } else {
                    // Keyless-capable kinds let the user name the provider so several can coexist; seed
                    // the field with the canonical token as an editable suggestion.
                    wizard.step = WizardStep::ProviderId;
                    if wizard.id.is_empty() {
                        wizard.id = wizard.provider_id();
                    }
                }
            }
            _ => {}
        }
        return vec![];
    }

    match key.code {
        // Ctrl+V pastes into the masked field via the clipboard (which routes to the wizard, never the
        // plaintext composer), instead of inserting a literal 'v'. Critical on the API-key step: a
        // pasted key would otherwise be silently corrupted, and the field is masked so it is invisible.
        Key::Char('v') if key.ctrl && !key.alt => vec![Effect::PasteClipboard],
        // Only a plain character types into the field; any other chord is ignored so it cannot corrupt
        // the input (e.g. Ctrl+A inserting 'a').
        Key::Char(c) if !key.ctrl && !key.alt => {
            wizard.push_char(c);
            vec![]
        }
        Key::Backspace => {
            wizard.backspace();
            vec![]
        }
        Key::Enter => advance_wizard(model),
        _ => vec![],
    }
}

/// Advance a text step on Enter: validate the required fields, move to the next step, or finalize. A
/// blank required field (model, API key) keeps the wizard on the step rather than producing an invalid
/// provider. Each arm re-borrows `model.wizard` freshly, so the finalize step can `take` it without a
/// borrow conflict (and without an `expect`).
fn advance_wizard(model: &mut Model) -> Vec<Effect> {
    let Some(step) = model.wizard.as_ref().map(|w| w.step) else {
        return vec![];
    };
    match step {
        WizardStep::Kind => vec![],
        WizardStep::ProviderId => {
            if let Some(wizard) = model.wizard.as_mut() {
                // The id is sanitized at finalize (provider_id) and falls back to the canonical token, so
                // a blank field is acceptable here; seed the base URL for the next step.
                wizard.step = WizardStep::BaseUrl;
                if wizard.base_url.trim().is_empty() {
                    wizard.base_url = wizard.kind().default_base_url().to_string();
                }
            }
            vec![]
        }
        WizardStep::BaseUrl => {
            let Some(wizard) = model.wizard.as_mut() else {
                return vec![];
            };
            if wizard.base_url.trim().is_empty() {
                // Vendor kinds default their endpoint; compatible/custom have none, so a blank base URL
                // stays on the step rather than saving an unusable endpoint that POSTs to "/chat/completions".
                let default = wizard.kind().default_base_url();
                if default.is_empty() {
                    return vec![]; // a base URL is required for compatible/custom
                }
                wizard.base_url = default.to_string();
            }
            wizard.step = WizardStep::Model;
            vec![]
        }
        WizardStep::Model => {
            let Some(wizard) = model.wizard.as_mut() else {
                return vec![];
            };
            if wizard.model.trim().is_empty() {
                return vec![]; // a model is required
            }
            wizard.step = WizardStep::ExtraModels;
            vec![]
        }
        WizardStep::ExtraModels => {
            if let Some(wizard) = model.wizard.as_mut() {
                wizard.step = WizardStep::ApiKey;
            }
            vec![]
        }
        WizardStep::ApiKey => {
            // The key is optional for keyless-capable kinds and required for vendor kinds; its presence
            // decides the auth method, so the two can never disagree.
            let has_key = model
                .wizard
                .as_ref()
                .is_some_and(|w| !w.api_key.trim().is_empty());
            let key_required = model.wizard.as_ref().is_some_and(|w| w.key_required());
            if !has_key && key_required {
                return vec![]; // a vendor kind requires a key
            }
            // Finalize: take the wizard, stage the key as a Secret (out of the effect) only when present,
            // emit SaveProvider. `mem::take` extracts the key without moving the field out of the `Drop`
            // type; the emptied buffer is then zeroized when `wizard` drops at the end of this scope.
            let Some(mut wizard) = model.wizard.take() else {
                return vec![];
            };
            let base_url = if wizard.base_url.trim().is_empty() {
                wizard.kind().default_base_url().to_string()
            } else {
                wizard.base_url.trim().to_string()
            };
            let auth = if has_key {
                AuthMethod::ApiKey
            } else {
                AuthMethod::None
            };
            let effect = Effect::SaveProvider {
                id: wizard.provider_id(),
                kind: wizard.kind(),
                base_url,
                model: wizard.model.trim().to_string(),
                models: wizard.models(),
                auth,
            };
            if has_key {
                model.pending_credential = Some(Secret::new(std::mem::take(&mut wizard.api_key)));
            }
            vec![effect]
        }
    }
}
