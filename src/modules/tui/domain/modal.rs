/// The options shown for a tool-call confirmation, in display order. `PendingApproval.selected` indexes
/// this list; the keymap maps the chosen index to an approval decision (option 1 also switches to auto).
pub const APPROVAL_OPTIONS: [&str; 3] = ["Sim", "Sim, e não perguntar de novo (modo auto)", "Não"];

/// A tool-call (or runaway-checkpoint) confirmation awaiting the user's answer. Pure data — the reply
/// channel lives in the runtime, since the engine handles approvals one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub prompt: String,
    pub default_accept: bool,
    /// The highlighted option index into `APPROVAL_OPTIONS`.
    pub selected: usize,
}

impl PendingApproval {
    /// A new pending approval, highlighting the option matching the default (accept → "Sim",
    /// decline → "Não").
    pub fn new(prompt: String, default_accept: bool) -> Self {
        let selected = if default_accept {
            0
        } else {
            APPROVAL_OPTIONS.len() - 1
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
/// unattended in auto), keep refining it, or leave plan mode.
pub const PLAN_OPTIONS: [&str; 4] = [
    "Executar o plano",
    "Executar o plano em modo auto",
    "Continuar planejando",
    "Cancelar (sair do modo plan)",
];

/// A finished plan-mode turn awaiting the user's decision. The plan itself is the assistant's last
/// transcript item; this only tracks which action is highlighted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingPlan {
    /// The highlighted option index into `PLAN_OPTIONS`.
    pub selected: usize,
}
