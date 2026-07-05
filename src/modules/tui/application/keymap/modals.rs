use super::*;

/// A resolved approval choice from a key press while a confirmation is pending.
enum Choice {
    /// Pick a named option from [`ApprovalOption::ALL`].
    Option(ApprovalOption),
    /// Decline just this call (Esc / 'n').
    Decline,
    /// End the whole session (Ctrl+C).
    Abort,
}

/// Map a typed digit ('1'..) to the option at that 1-based position, if it is in range. Driven by
/// `ALL.len()` so adding or removing an option keeps the shortcuts aligned with the rendered list.
fn approval_option_for_digit(c: char) -> Option<ApprovalOption> {
    let digit = c.to_digit(10)? as usize;
    if (1..=ApprovalOption::ALL.len()).contains(&digit) {
        ApprovalOption::from_index(digit - 1)
    } else {
        None
    }
}

/// Answer the pending confirmation: arrows move the highlight (wrapping, like every modal), Enter takes
/// the highlighted option, digits/letters jump straight to one, Esc declines this call, and Ctrl+C aborts
/// the session. `ApproveAuto` ("Sim, e não perguntar de novo") also switches the session to auto mode.
pub(super) fn on_approval_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    // Navigation moves the highlight without answering.
    if let Some(pending) = model.pending_approval.as_mut() {
        match key.code {
            Key::Up => {
                pending.selected = wrapping_step(pending.selected, -1, ApprovalOption::ALL.len());
                return vec![];
            }
            Key::Down => {
                pending.selected = wrapping_step(pending.selected, 1, ApprovalOption::ALL.len());
                return vec![];
            }
            _ => {}
        }
    }

    let selected = model.pending_approval.as_ref().map_or(0, |p| p.selected);
    let choice = match key.code {
        // Ctrl+C while an approval is pending answers the reply channel with Aborted — the engine future is
        // blocked waiting on it, so a bare Quit here would strand it; the session ends via Aborted instead.
        Key::Char('c') if key.ctrl => Some(Choice::Abort),
        Key::Esc => Some(Choice::Decline),
        Key::Enter => ApprovalOption::from_index(selected).map(Choice::Option),
        Key::Char(c) if c.is_ascii_digit() => approval_option_for_digit(c).map(Choice::Option),
        Key::Char(c) => match c.to_ascii_lowercase() {
            's' | 'y' => Some(Choice::Option(ApprovalOption::Approve)),
            'n' => Some(Choice::Decline),
            _ => None,
        },
        _ => None,
    };
    let Some(choice) = choice else {
        return vec![];
    };

    let Some(pending) = model.pending_approval.take() else {
        return vec![];
    };
    let (decision, switch_auto) = match choice {
        Choice::Abort => (Approval::Aborted, false),
        Choice::Decline => (Approval::Declined, false),
        Choice::Option(ApprovalOption::Approve) => (Approval::Approved, false),
        Choice::Option(ApprovalOption::ApproveAuto) => (Approval::ApprovedAuto, true),
        Choice::Option(ApprovalOption::Decline) => (Approval::Declined, false),
    };
    let (level, label) = match decision {
        Approval::Approved | Approval::ApprovedAuto => {
            (NoticeLevel::Info, format!("✓ {}", pending.action()))
        }
        Approval::Declined => (
            NoticeLevel::Error,
            format!("✗ recusado: {}", pending.action()),
        ),
        Approval::Aborted => (NoticeLevel::Error, "✗ sessão encerrada".to_string()),
    };
    model.notify(level, label);
    // `ApprovedAuto` already runs the rest of this turn unattended; also make auto the standing mode
    // so later turns no longer prompt either.
    if switch_auto {
        model.approval_mode = ApprovalMode::Auto;
        model.notify_info("✓ modo auto ativo");
    }
    vec![Effect::AnswerApproval(decision)]
}

/// Drive the plan box shown after a plan-mode turn: arrows move the highlight, Enter/digits pick an
/// option, Esc cancels, Ctrl+C quits. "Executar" emits `ApprovePlan` (in default or auto mode);
/// "Continuar" just closes the box (staying in plan mode); "Cancelar" closes it and returns to default.
pub(super) fn on_plan_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if let Some(plan) = model.pending_plan.as_mut() {
        match key.code {
            Key::Up if key.shift => {
                plan.scroll = plan.scroll.saturating_sub(1);
                return vec![];
            }
            Key::Down if key.shift => {
                plan.scroll = plan.scroll.saturating_add(1);
                return vec![];
            }
            Key::Up => {
                plan.selected = wrapping_step(plan.selected, -1, PlanOption::ALL.len());
                return vec![];
            }
            Key::Down => {
                plan.selected = wrapping_step(plan.selected, 1, PlanOption::ALL.len());
                return vec![];
            }
            Key::PageUp => {
                plan.scroll = plan.scroll.saturating_sub(10);
                return vec![];
            }
            Key::PageDown => {
                plan.scroll = plan.scroll.saturating_add(10);
                return vec![];
            }
            _ => {}
        }
    }

    if key.ctrl && key.code == Key::Char('c') {
        model.pending_plan = None;
        model.should_quit = true;
        return vec![Effect::Quit];
    }

    let selected = model.pending_plan.as_ref().map_or(0, |p| p.selected);
    let option = match key.code {
        Key::Enter => PlanOption::from_index(selected),
        Key::Char(c) if c.is_ascii_digit() => plan_option_for_digit(c),
        Key::Esc => Some(PlanOption::Cancel),
        _ => return vec![],
    };
    let Some(option) = option else {
        return vec![];
    };

    model.pending_plan = None;
    match option {
        PlanOption::Execute => vec![Effect::ApprovePlan(ApprovalMode::Default)],
        PlanOption::ExecuteAuto => vec![Effect::ApprovePlan(ApprovalMode::Auto)],
        PlanOption::KeepPlanning => vec![],
        PlanOption::Cancel => {
            model.approval_mode = ApprovalMode::Default;
            model.notify_info("modo plan cancelado");
            vec![]
        }
    }
}

/// Map a typed digit ('1'..) to the plan option at that 1-based position, if it is in range. Driven by
/// `ALL.len()` so the shortcuts stay aligned with the rendered list.
fn plan_option_for_digit(c: char) -> Option<PlanOption> {
    let digit = c.to_digit(10)? as usize;
    if (1..=PlanOption::ALL.len()).contains(&digit) {
        PlanOption::from_index(digit - 1)
    } else {
        None
    }
}

/// Drive an open `/models` / `/effort` picker: arrows move the highlight, Enter/digits pick a row, Esc
/// closes it, Ctrl+C quits. Enter on a `Models` picker emits `SetModel`; on `Effort`, the row index maps
/// back to `Effort::ALL` for `SetEffort`. The runtime applies the swap and persists it.
pub(super) fn on_picker_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if key.ctrl && key.code == Key::Char('c') {
        model.picker = None;
        model.should_quit = true;
        return vec![Effect::Quit];
    }

    let mut query_changed = false;
    if let Some(picker) = model.picker.as_mut() {
        match key.code {
            Key::Up => {
                picker.move_cursor(-1);
                return vec![];
            }
            Key::Down => {
                picker.move_cursor(1);
                return vec![];
            }
            Key::Esc => {
                model.picker = None;
                return vec![];
            }
            Key::Backspace => {
                picker.query.pop();
                picker.selected = 0;
                query_changed = true;
            }
            Key::Char(c) if !key.ctrl && !key.alt => {
                picker.query.push(c);
                picker.selected = 0;
                query_changed = true;
            }
            Key::Enter => {}
            _ => return vec![],
        }
    }

    if query_changed {
        return vec![];
    }

    let picker = match &model.picker {
        Some(p) => p,
        None => return vec![],
    };

    let filtered = picker.filtered_options();
    let index = match key.code {
        Key::Enter => {
            if picker.selected < filtered.len() {
                filtered[picker.selected].0
            } else {
                return vec![];
            }
        }
        _ => return vec![],
    };

    let Some(picker) = model.picker.take() else {
        return vec![];
    };
    match picker.kind {
        PickerKind::Models => match picker.options.get(index) {
            Some(model_id) => vec![Effect::SetModel(model_id.clone())],
            None => vec![],
        },
        PickerKind::Effort => match Effort::ALL.get(index) {
            // The index comes from the Effort picker, itself built from `Effort::ALL`, so it is always in
            // range; mirror the Models/Sessions arms — a no-op on the unreachable miss, never a silent
            // `unwrap_or_default` that could quietly apply the wrong effort.
            Some(effort) => vec![Effect::SetEffort(*effort)],
            None => vec![],
        },
        PickerKind::Provider => {
            // The configured ids come first; the last row (`index == providers.len()`) is the
            // "+ adicionar" sentinel, which opens the add wizard instead of switching.
            if index < model.providers.len() {
                let id = match picker.options.get(index) {
                    Some(id) => id.clone(),
                    None => return vec![],
                };
                let detail = model
                    .provider_profiles
                    .iter()
                    .find(|p| p.id == id)
                    .map(provider_detail_line)
                    .unwrap_or_default();
                model.picker = Some(Picker::new(
                    PickerKind::ProviderAction(id),
                    "provider",
                    detail,
                    vec!["Ativar".into(), "Editar".into(), "Remover".into()],
                    0,
                ));
                vec![]
            } else {
                model.wizard = Some(ProviderWizard::new());
                vec![]
            }
        }
        PickerKind::ProviderAction(ref id) => {
            let id = id.clone();
            match index {
                0 => vec![Effect::SetProvider(id)],
                1 => {
                    if let Some(profile) = model.provider_profiles.iter().find(|p| p.id == id) {
                        model.wizard = Some(ProviderWizard::from_profile(profile));
                    }
                    vec![]
                }
                _ => {
                    model.picker = Some(Picker::new(
                        PickerKind::ProviderDeleteConfirm(id),
                        "confirmar",
                        "esta ação não pode ser desfeita",
                        vec!["Sim, remover".into(), "Cancelar".into()],
                        1,
                    ));
                    vec![]
                }
            }
        }
        PickerKind::ProviderDeleteConfirm(ref id) => {
            if index == 0 {
                vec![Effect::DeleteProvider(id.clone())]
            } else {
                vec![]
            }
        }
        PickerKind::Sessions => match model.session_ids.get(index) {
            // The labels are display titles; the id comes from the parallel `session_ids`, filled by
            // the runtime when it opened the picker.
            Some(id) => vec![Effect::OpenSession(id.clone())],
            None => vec![],
        },
    }
}

/// One-line summary of a provider profile shown as the action picker's description line.
fn provider_detail_line(p: &ProviderProfile) -> String {
    let kind = format!("{:?}", p.kind).to_ascii_lowercase();
    let thinking_tag = match p.thinking {
        Some(true) => " · thinking: sim",
        Some(false) => " · thinking: não",
        None => "",
    };
    format!(
        "[{}] {} · {} · {}{}",
        kind,
        p.base_url,
        p.model,
        p.auth.as_wire(),
        thinking_tag,
    )
}
