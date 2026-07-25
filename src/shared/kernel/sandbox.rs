//! These live in the kernel so the sync trust gate can reason over them without depending on
//! `tools`/`infra`, keeping a future `sync/domain` gate pure. Every `Deserialize` here is
//! forward-compatible: an unrecognized value maps to the *safe* variant, never a silent weakening.

use serde::Deserialize;
use serde::de::Deserializer;

/// The OS-confinement requirement for `run_command`, ranked so the trust gate can flag *any* relaxation,
/// not only the extreme `→ Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// `KIRI_SANDBOX=off`.
    Off,
    /// The default: use the platform adapter where available, but do not require it.
    Os,
    /// Refuse `run_command` when no OS sandbox is available.
    Require,
}

impl SandboxMode {
    /// Every accepted token, so a loader can tell "the user typed something we do not understand" from
    /// "the user typed `os`" — both land on [`SandboxMode::Os`], and only the first deserves a warning.
    /// Kept beside [`SandboxMode::from_config`], the one place that assigns meaning to these strings, so
    /// a new mode cannot be recognized in one and unknown in the other.
    pub const RECOGNIZED: &'static [&'static str] = &["off", "os", "require"];

    /// The trust gate flags a strictly-lower incoming rank (`Require > Os > Off`).
    pub fn rank(self) -> u8 {
        match self {
            SandboxMode::Off => 0,
            SandboxMode::Os => 1,
            SandboxMode::Require => 2,
        }
    }

    /// Unrecognized or absent is `Os`, never a silent downgrade to `Off`.
    pub fn from_config(raw: Option<&str>) -> SandboxMode {
        match raw {
            Some("off") => SandboxMode::Off,
            Some("require") => SandboxMode::Require,
            _ => SandboxMode::Os,
        }
    }
}

impl<'de> Deserialize<'de> for SandboxMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(SandboxMode::from_config(Some(raw.as_str())))
    }
}

/// The base network stance for a confined `run_command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStance {
    Deny,
    Allow,
}

impl NetworkStance {
    /// See [`SandboxMode::RECOGNIZED`] — same purpose, same reason for living next to `from_config`.
    pub const RECOGNIZED: &'static [&'static str] = &["allow", "deny"];

    /// Only `allow` widens; anything else, including absent, is `Deny` — never a silent widening.
    pub fn from_config(raw: Option<&str>) -> NetworkStance {
        match raw {
            Some("allow") => NetworkStance::Allow,
            _ => NetworkStance::Deny,
        }
    }
}

impl<'de> Deserialize<'de> for NetworkStance {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(NetworkStance::from_config(Some(raw.as_str())))
    }
}

/// The resolved policy the `tools` layer consumes; the config resolvers map a [`NetworkStance`] to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    Deny,
    Allow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recognized_token_has_its_own_from_config_arm() {
        // The drift guard `RECOGNIZED` exists for: a token listed here but missing an arm in `from_config`
        // silently falls through to the safe default, which would make the loader accept it while treating
        // it as meaningless — the same silent no-op the warning is supposed to expose.
        let modes: Vec<SandboxMode> = SandboxMode::RECOGNIZED
            .iter()
            .map(|token| SandboxMode::from_config(Some(token)))
            .collect();
        assert_eq!(
            modes,
            vec![SandboxMode::Off, SandboxMode::Os, SandboxMode::Require],
            "RECOGNIZED and from_config disagree"
        );

        let stances: Vec<NetworkStance> = NetworkStance::RECOGNIZED
            .iter()
            .map(|token| NetworkStance::from_config(Some(token)))
            .collect();
        assert_eq!(
            stances,
            vec![NetworkStance::Allow, NetworkStance::Deny],
            "RECOGNIZED and from_config disagree"
        );
    }

    #[test]
    fn sandbox_mode_from_config_none_and_unknown_are_os() {
        assert_eq!(SandboxMode::from_config(None), SandboxMode::Os);
        assert_eq!(SandboxMode::from_config(Some("bogus")), SandboxMode::Os);
        assert_eq!(SandboxMode::from_config(Some("os")), SandboxMode::Os);
        assert_eq!(SandboxMode::from_config(Some("off")), SandboxMode::Off);
        assert_eq!(
            SandboxMode::from_config(Some("require")),
            SandboxMode::Require
        );
    }

    #[test]
    fn sandbox_mode_rank_orders_require_os_off() {
        assert!(SandboxMode::Require.rank() > SandboxMode::Os.rank());
        assert!(SandboxMode::Os.rank() > SandboxMode::Off.rank());
    }

    #[test]
    fn sandbox_mode_deserialize_is_forward_compatible() {
        let mode: SandboxMode = serde_json::from_str("\"future-mode\"").unwrap();
        assert_eq!(
            mode,
            SandboxMode::Os,
            "unknown deserializes to the os default"
        );
        let off: SandboxMode = serde_json::from_str("\"off\"").unwrap();
        assert_eq!(off, SandboxMode::Off);
    }

    #[test]
    fn network_stance_from_config_defaults_deny() {
        assert_eq!(NetworkStance::from_config(None), NetworkStance::Deny);
        assert_eq!(
            NetworkStance::from_config(Some("bogus")),
            NetworkStance::Deny
        );
        assert_eq!(
            NetworkStance::from_config(Some("deny")),
            NetworkStance::Deny
        );
        assert_eq!(
            NetworkStance::from_config(Some("allow")),
            NetworkStance::Allow
        );
    }

    #[test]
    fn network_stance_deserialize_defaults_deny() {
        let stance: NetworkStance = serde_json::from_str("\"future-stance\"").unwrap();
        assert_eq!(stance, NetworkStance::Deny, "unknown deserializes to deny");
        let allow: NetworkStance = serde_json::from_str("\"allow\"").unwrap();
        assert_eq!(allow, NetworkStance::Allow);
    }
}
