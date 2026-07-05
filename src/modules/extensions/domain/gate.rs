//! Pure resolution logic for ADR 0021's trust gate: whether a discovered active capability (hooks, MCP,
//! future sub-agents) may execute. Global-layer capabilities are always approved — they come from the
//! trusted `~/.kiri/` tree the user themselves authored. Project-layer ones need an explicit prior
//! approval recorded for their current content; the caller looks that up in the trust store
//! (`infrastructure::trust_store`) and passes the boolean in here — this module has no I/O of its own.

use crate::modules::extensions::domain::scope::Layer;

/// The resolved state of one active-capability gate.
// ponytail: no caller yet — the first active-capability type (hooks) lands in Fase 4 and resolves each
// discovered hook through this before letting it execute.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    Approved,
    Pending,
}

/// Resolve the gate state for a capability discovered at `layer`. Global is always `Approved`; project is
/// `Approved` only when `previously_approved` (a trust-store hit for the capability's current content
/// hash) is `true`, else `Pending` — the runtime surfaces a `BootNotice` and asks the user.
#[allow(dead_code)]
pub fn resolve(layer: Layer, previously_approved: bool) -> GateState {
    match layer {
        Layer::Global => GateState::Approved,
        Layer::Project if previously_approved => GateState::Approved,
        Layer::Project => GateState::Pending,
    }
}

/// A short, stable content hash (blake3, first 16 hex chars) for TOFU comparison: the trust store keys an
/// approval to this hash, so editing a project capability's content after approval reverts it to pending.
#[allow(dead_code)]
pub fn content_hash(content: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(content.as_bytes());
    hasher.finalize().to_hex().as_str()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_is_always_approved() {
        assert_eq!(resolve(Layer::Global, false), GateState::Approved);
        assert_eq!(resolve(Layer::Global, true), GateState::Approved);
    }

    #[test]
    fn project_is_approved_only_when_previously_approved() {
        assert_eq!(resolve(Layer::Project, true), GateState::Approved);
        assert_eq!(resolve(Layer::Project, false), GateState::Pending);
    }

    #[test]
    fn content_hash_is_deterministic_and_sensitive_to_change() {
        let a = content_hash("run rm -rf /");
        let b = content_hash("run rm -rf /");
        let c = content_hash("run echo hi");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }
}
