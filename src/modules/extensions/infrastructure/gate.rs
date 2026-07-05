//! Active-capability gate: project-layer resources that represent execution capability (hooks, MCP,
//! sub-agents) are discovered but kept **disabled** until the user explicitly approves them. Resolves
//! to an `ActiveGate` the composition root passes to the runtime (a `BootNotice` list + an approval
//! channel per capability). Delegates to the existing onboarding/approval machinery.

/// The resolved state of one active-capability gate: whether it was approved or is still pending.
/// Global capabilities are always approved; project ones default to `Pending`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateState {
    Approved,
    Pending,
}

/// The active-capability gate resolver, built by the composition root. For now a stub: all project
/// active capabilities are *discovered* but kept pending. Fase 4/5 will add the per-capability approval
/// channel + `BootNotice` generation.
pub struct ActiveGate;

impl ActiveGate {
    /// Resolve the gate state for a capability discovered at `layer`. Global capabilities are always
    /// approved; project ones are kept pending (the runtime surfaces a boot notice and asks the user).
    pub fn resolve(layer: crate::modules::extensions::domain::scope::Layer) -> GateState {
        match layer {
            crate::modules::extensions::domain::scope::Layer::Global => GateState::Approved,
            crate::modules::extensions::domain::scope::Layer::Project => GateState::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::extensions::domain::scope::Layer;

    #[test]
    fn global_is_always_approved() {
        assert_eq!(ActiveGate::resolve(Layer::Global), GateState::Approved);
    }

    #[test]
    fn project_is_always_pending() {
        assert_eq!(ActiveGate::resolve(Layer::Project), GateState::Pending);
    }
}
