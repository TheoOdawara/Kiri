/// The options shown for a tool-call confirmation, in display order. The named variants are the single
/// source for both the labels and the index→decision mapping, so reordering them can never desync the
/// digit-key shortcuts from their meaning. `PendingApproval.selected` indexes [`ApprovalOption::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOption {
    /// Approve just this call.
    Approve,
    /// Approve this call and switch the session to auto mode (stop prompting).
    ApproveAuto,
    /// Decline this call.
    Decline,
}

impl ApprovalOption {
    /// The options in display order; an index into this slice is what `PendingApproval.selected` holds.
    pub const ALL: [ApprovalOption; 3] = [
        ApprovalOption::Approve,
        ApprovalOption::ApproveAuto,
        ApprovalOption::Decline,
    ];

    /// The pt-BR label rendered for this option in the confirmation box.
    pub fn label(self) -> &'static str {
        match self {
            ApprovalOption::Approve => "Sim",
            ApprovalOption::ApproveAuto => "Sim, e não perguntar de novo (modo auto)",
            ApprovalOption::Decline => "Não",
        }
    }

    /// Resolve a highlighted index back to its named option, if in range.
    pub fn from_index(index: usize) -> Option<ApprovalOption> {
        ApprovalOption::ALL.get(index).copied()
    }
}

/// A tool-call (or runaway-checkpoint) confirmation awaiting the user's answer. Pure data — the reply
/// channel lives in the runtime, since the engine handles approvals one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub prompt: String,
    pub default_accept: bool,
    /// The highlighted option index into [`ApprovalOption::ALL`].
    pub selected: usize,
}

impl PendingApproval {
    /// A new pending approval, highlighting the option matching the default (accept → "Sim",
    /// decline → "Não").
    pub fn new(prompt: String, default_accept: bool) -> Self {
        let selected = if default_accept {
            0
        } else {
            ApprovalOption::ALL.len() - 1
        };
        Self {
            prompt,
            default_accept,
            selected,
        }
    }

    /// The confirmation question without the trailing `[S/n]`/`[s/N]` hint — the rich box shows the
    /// selectable options instead of the inline default.
    pub fn action(&self) -> &str {
        self.prompt
            .trim_end()
            .trim_end_matches("[S/n]")
            .trim_end_matches("[s/N]")
            .trim_end()
    }
}

/// The options shown when a plan-mode turn finishes: run the plan (confirming each step or fully
/// unattended in auto), keep refining it, or leave plan mode. Named for the same reason as
/// [`ApprovalOption`] — the variant, not a positional index, carries the meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanOption {
    /// Execute the plan confirming each step (leave plan mode for default mode).
    Execute,
    /// Execute the plan unattended (leave plan mode for auto mode).
    ExecuteAuto,
    /// Close the box and stay in plan mode for more input.
    KeepPlanning,
    /// Leave plan mode without executing.
    Cancel,
}

impl PlanOption {
    /// The options in display order; an index into this slice is what `PendingPlan.selected` holds.
    pub const ALL: [PlanOption; 4] = [
        PlanOption::Execute,
        PlanOption::ExecuteAuto,
        PlanOption::KeepPlanning,
        PlanOption::Cancel,
    ];

    /// The pt-BR label rendered for this option in the plan box.
    pub fn label(self) -> &'static str {
        match self {
            PlanOption::Execute => "Executar o plano",
            PlanOption::ExecuteAuto => "Executar o plano em modo auto",
            PlanOption::KeepPlanning => "Continuar planejando",
            PlanOption::Cancel => "Cancelar (sair do modo plan)",
        }
    }

    /// Resolve a highlighted index back to its named option, if in range.
    pub fn from_index(index: usize) -> Option<PlanOption> {
        PlanOption::ALL.get(index).copied()
    }
}

/// A finished plan-mode turn awaiting the user's decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingPlan {
    /// The proposed plan text.
    pub plan: String,
    /// The highlighted option index into [`PlanOption::ALL`].
    pub selected: usize,
    /// The current scroll offset in lines.
    pub scroll: usize,
}
