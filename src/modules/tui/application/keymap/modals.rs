use super::*;

/// A resolved approval choice from a key press while a confirmation is pending.
enum Choice {
    /// Pick option `usize` from `APPROVAL_OPTIONS`.
    Option(usize),
    /// Decline just this call (Esc / 'n').
    Decline,
    /// End the whole session (Ctrl+C).
    Abort,
}

/// Answer the pending confirmation: arrows move the highlight, Enter takes the highlighted option,
/// digits/letters jump straight to one, Esc declines this call, and Ctrl+C aborts the session. Option 2
/// ("Sim, e não perguntar de novo") also switches the session to auto mode.
pub(super) fn on_approval_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    // Navigation moves the highlight without answering.
    if let Some(pending) = model.pending_approval.as_mut() {
        match key.code {
            Key::Up => {
                pending.selected = pending.selected.saturating_sub(1);
                return vec![];
            }
            Key::Down => {
                pending.selected = (pending.selected + 1).min(APPROVAL_OPTIONS.len() - 1);
                return vec![];
            }
            _ => {}
        }
    }

    let selected = model.pending_approval.as_ref().map_or(0, |p| p.selected);
    let choice = match key.code {
        Key::Char('c') if key.ctrl => Some(Choice::Abort),
        Key::Esc => Some(Choice::Decline),
        Key::Enter => Some(Choice::Option(selected)),
        Key::Char('1') => Some(Choice::Option(0)),
        Key::Char('2') => Some(Choice::Option(1)),
        Key::Char('3') => Some(Choice::Option(2)),
        Key::Char(c) => match c.to_ascii_lowercase() {
            's' | 'y' => Some(Choice::Option(0)),
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
        Choice::Option(0) => (Approval::Approved, false),
        Choice::Option(1) => (Approval::ApprovedAuto, true),
        Choice::Option(_) => (Approval::Declined, false),
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
    model.transcript.push(TranscriptItem::Notice(level, label));
    // `ApprovedAuto` already runs the rest of this turn unattended; also make auto the standing mode
    // so later turns no longer prompt either.
    if switch_auto {
        model.approval_mode = ApprovalMode::Auto;
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "✓ modo auto ativo".to_string(),
        ));
    }
    vec![Effect::AnswerApproval(decision)]
}

/// Drive the plan box shown after a plan-mode turn: arrows move the highlight, Enter/digits pick an
/// option, Esc cancels, Ctrl+C quits. "Executar" emits `ApprovePlan` (in default or auto mode);
/// "Continuar" just closes the box (staying in plan mode); "Cancelar" closes it and returns to default.
pub(super) fn on_plan_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
    if let Some(plan) = model.pending_plan.as_mut() {
        match key.code {
            Key::Up => {
                plan.selected = plan.selected.saturating_sub(1);
                return vec![];
            }
            Key::Down => {
                plan.selected = (plan.selected + 1).min(PLAN_OPTIONS.len() - 1);
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
    let index = match key.code {
        Key::Enter => selected,
        Key::Char('1') => 0,
        Key::Char('2') => 1,
        Key::Char('3') => 2,
        Key::Char('4') => 3,
        Key::Esc => 3,
        _ => return vec![],
    };

    model.pending_plan = None;
    match index {
        // Execute the plan confirming each step: the runtime leaves plan mode for default mode.
        0 => vec![Effect::ApprovePlan(ApprovalMode::Default)],
        // Execute the plan unattended: the runtime leaves plan mode for auto mode.
        1 => vec![Effect::ApprovePlan(ApprovalMode::Auto)],
        // Keep planning: close the box and stay in plan mode for more input.
        2 => vec![],
        // Cancel: leave plan mode without executing.
        _ => {
            model.approval_mode = ApprovalMode::Default;
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                "modo plan cancelado".to_string(),
            ));
            vec![]
        }
    }
}

/// Drive an open `/models` / `/effort` picker: arrows move the highlight, Enter/digits pick a row, Esc
/// closes it, Ctrl+C quits. Enter on a `Models` picker emits `SetModel`; on `Effort`, the row index maps
/// back to `Effort::ALL` for `SetEffort`. The runtime applies the swap and persists it.
pub(super) fn on_picker_key(model: &mut Model, key: KeyPress) -> Vec<Effect> {
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
            _ => {}
        }
    }

    if key.ctrl && key.code == Key::Char('c') {
        model.picker = None;
        model.should_quit = true;
        return vec![Effect::Quit];
    }

    let selected = model.picker.as_ref().map_or(0, |p| p.selected);
    let option_count = model.picker.as_ref().map_or(0, |p| p.options.len());
    let index = match key.code {
        Key::Enter => selected,
        Key::Esc => {
            model.picker = None;
            return vec![];
        }
        Key::Char(c) if c.is_ascii_digit() => {
            let digit = c.to_digit(10).unwrap_or(0) as usize;
            if digit >= 1 && digit <= option_count {
                digit - 1
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
        PickerKind::Effort => {
            let effort = Effort::ALL.get(index).copied().unwrap_or_default();
            vec![Effect::SetEffort(effort)]
        }
        PickerKind::Provider => {
            // The configured ids come first; the last row (`index == providers.len()`) is the
            // "+ adicionar" sentinel, which opens the add wizard instead of switching.
            if index < model.providers.len() {
                match picker.options.get(index) {
                    Some(id) => vec![Effect::SetProvider(id.clone())],
                    None => vec![],
                }
            } else {
                model.wizard = Some(ProviderWizard::new());
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
