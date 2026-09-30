use super::*;

/// Drive the add-provider wizard. The `Kind` step uses arrows + Enter; the text steps use a full
/// `InputBuffer` draft (cursor, wrap, selection, undo) via `field_chords` + `feed_key`. The final
/// `ApiKey` step finalizes — staging the key in `Model::pending_credential` (a `Secret`) and emitting
/// `SaveProvider` (no secret). Esc cancels any step; Ctrl+C copies a selection or quits.
pub(super) fn on_wizard_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    // Ctrl+C: copy selection from the draft when present; otherwise quit (modal cancel+exit).
    if key.ctrl && !key.alt && key.code == Key::Char('c') {
        if let Some(wizard) = model.wizard.as_mut()
            && let Some(text) = wizard.draft.copy_selection()
        {
            return vec![Effect::CopyToClipboard(text)];
        }
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
        model.notify_info(message);
        return vec![];
    }

    let Some(wizard) = model.wizard.as_mut() else {
        return vec![];
    };

    // The Kind and Thinking steps are choosers; the rest are text fields.
    if wizard.step == WizardStep::Kind {
        match key.code {
            Key::Up => wizard.move_kind(-1),
            Key::Down => wizard.move_kind(1),
            Key::Enter => {
                // Seed the thinking toggle from the kind's default now that the kind is confirmed.
                wizard.thinking = wizard.kind().thinking_default();
                if wizard.key_required() {
                    // Vendor kinds use a canonical id; go straight to the base URL, seeded with the
                    // kind's default so the common case is one keystroke (Enter).
                    if wizard.base_url.is_empty() {
                        wizard.base_url = wizard.kind().default_base_url().to_string();
                    }
                    wizard.go_to_step(WizardStep::BaseUrl);
                } else {
                    // Keyless-capable kinds let the user name the provider so several can coexist; seed
                    // the field with the canonical token as an editable suggestion.
                    if wizard.id.is_empty() {
                        wizard.id = wizard.provider_id();
                    }
                    wizard.go_to_step(WizardStep::ProviderId);
                }
            }
            _ => {}
        }
        return vec![];
    }

    if wizard.step == WizardStep::Thinking {
        match key.code {
            Key::Up | Key::Down => wizard.toggle_thinking(),
            Key::Enter => {
                wizard.go_to_step(WizardStep::ApiKey);
            }
            _ => {}
        }
        return vec![];
    }

    // Text steps: shared clipboard/undo chords, then ordinary editor input. Enter advances.
    if let Some(effects) = super::field_edit::field_chords(&mut wizard.draft, key) {
        wizard.commit_draft_to_field();
        return effects;
    }
    match key.code {
        Key::Enter => advance_wizard(model),
        // Do not insert a newline into the draft — Enter already advances. Fall through for everything
        // else (typing, deletion, cursor motion, word motion, Shift-selection).
        _ => {
            if let Some(wizard) = model.wizard.as_mut() {
                wizard.feed_key(key);
            }
            vec![]
        }
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
    // Always commit before reading fields (draft is authoritative while typing).
    if let Some(wizard) = model.wizard.as_mut() {
        wizard.commit_draft_to_field();
    }
    match step {
        WizardStep::Kind => vec![],
        WizardStep::ProviderId => {
            if let Some(wizard) = model.wizard.as_mut() {
                // The id is sanitized at finalize (provider_id) and falls back to the canonical token, so
                // a blank field is acceptable here; seed the base URL for the next step.
                if wizard.base_url.trim().is_empty() {
                    wizard.base_url = wizard.kind().default_base_url().to_string();
                }
                wizard.go_to_step(WizardStep::BaseUrl);
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
            wizard.go_to_step(WizardStep::Model);
            vec![]
        }
        WizardStep::Model => {
            let Some(wizard) = model.wizard.as_mut() else {
                return vec![];
            };
            if wizard.model.trim().is_empty() {
                return vec![]; // a model is required
            }
            wizard.go_to_step(WizardStep::ExtraModels);
            vec![]
        }
        WizardStep::ExtraModels => {
            if let Some(wizard) = model.wizard.as_mut() {
                // Skip a Sim/Não question that would be a no-op: a kind/model with no thinking
                // capability at all (e.g. Gemma 3 / bare gemma, or an unknown compatible model) goes
                // straight to the key step instead of asking about a toggle that does nothing.
                let next = if wizard.kind().thinking_capability(&wizard.model)
                    == ThinkingCapability::Unsupported
                {
                    wizard.thinking = false;
                    WizardStep::ApiKey
                } else {
                    WizardStep::Thinking
                };
                wizard.go_to_step(next);
            }
            vec![]
        }
        WizardStep::Thinking => vec![], // handled in on_wizard_key above; advance_wizard is not called here
        WizardStep::ApiKey => {
            // The key is optional for keyless-capable kinds and required for vendor kinds; its presence
            // decides the auth method, so the two can never disagree.
            let has_key = model
                .wizard
                .as_ref()
                .is_some_and(|w| !w.api_key.trim().is_empty());
            let key_required = model.wizard.as_ref().is_some_and(|w| w.key_required());
            let edit_mode = model.wizard.as_ref().is_some_and(|w| w.edit_mode);
            let had_key = model.wizard.as_ref().is_some_and(|w| w.had_key);
            // In edit mode a blank key means "keep the existing credential" — skip the require check.
            if !has_key && key_required && !edit_mode {
                return vec![]; // a vendor kind requires a key on first save
            }
            let keep_existing_key = edit_mode && !has_key;
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
            // A blank key in edit mode means "keep the existing credential" — but only if there was one.
            // Gating solely on `key_required` (a kind-level constant, false for compatible/custom) would
            // collapse a keyless-capable provider that DOES have a stored key to `AuthMethod::None` here,
            // silently deleting that key later (`apply_save_provider`'s `AuthMethod::None` arm never
            // consults `keep_existing_key`). `had_key` (captured from the profile being edited) closes
            // that gap.
            let auth = if has_key || (edit_mode && (key_required || had_key)) {
                AuthMethod::ApiKey
            } else {
                AuthMethod::None
            };
            let thinking = Some(wizard.thinking);
            let effect = Effect::SaveProvider {
                id: wizard.provider_id(),
                kind: wizard.kind(),
                base_url,
                model: wizard.model.trim().to_string(),
                models: wizard.models(),
                auth,
                thinking,
                keep_existing_key,
            };
            if has_key {
                model.pending_credential = Some(Secret::new(std::mem::take(&mut wizard.api_key)));
            }
            vec![effect]
        }
    }
}
