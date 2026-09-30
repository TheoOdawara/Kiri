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
