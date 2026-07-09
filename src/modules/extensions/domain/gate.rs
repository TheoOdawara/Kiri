//! ADR 0021's trust gate, resolved as pure data: the caller looks the approval up in the trust store and
//! passes the boolean in, so this module keeps no I/O of its own.

use crate::modules::extensions::domain::scope::Layer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    Approved,
    Pending,
}

/// Global and Bundled are trusted, so always `Approved`. Project needs a trust-store hit for its current
/// content hash; otherwise `Pending`, and the runtime asks the user.
pub fn resolve(layer: Layer, previously_approved: bool) -> GateState {
    match layer {
        Layer::Global | Layer::Bundled => GateState::Approved,
        Layer::Project if previously_approved => GateState::Approved,
        Layer::Project => GateState::Pending,
    }
}

/// A short, stable content hash (blake3, first 16 hex chars) for TOFU comparison: the trust store keys an
/// approval to this hash, so editing a project capability's content after approval reverts it to pending.
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
    fn bundled_is_always_approved() {
        assert_eq!(resolve(Layer::Bundled, false), GateState::Approved);
        assert_eq!(resolve(Layer::Bundled, true), GateState::Approved);
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
