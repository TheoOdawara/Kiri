//! Typed sandbox-policy primitives shared by the config resolvers (`shared/infra/config`) and the sync
//! trust gate (`sync`). They live in the kernel — like [`super::provider::AuthMethod`] — so the trust
//! gate can reason over them without depending on `tools`/`infra`, keeping a future `sync/domain` gate
//! pure. Each enum's `Deserialize` is forward-compatible: an unrecognized value maps to the *safe*
//! variant (never a silent weakening), and an absent value (`from_config(None)`) maps to the documented
//! default — the same precedence the config resolvers apply.

use serde::Deserialize;
use serde::de::Deserializer;

/// The OS-confinement requirement for `run_command`, ranked so the trust gate can flag *any* relaxation
/// — not only the extreme `→ Off`. Higher rank = stronger confinement (`Require > Os > Off`).
///
/// - `Off` — OS confinement disabled (`KIRI_SANDBOX=off`).
/// - `Os` — use the platform adapter where available (the default); no hard requirement.
/// - `Require` — refuse `run_command` when no OS sandbox is available.
///
/// `Deserialize`/`from_config` map an unrecognized or absent value to `Os` (the config default), so a
/// forward-version or malformed value is never read as a silent downgrade to `Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    Off,
    Os,
    Require,
}

impl SandboxMode {
    /// Confinement strength. The trust gate flags a strictly-lower incoming rank (`Require > Os > Off`).
    pub fn rank(self) -> u8 {
        match self {
            SandboxMode::Off => 0,
            SandboxMode::Os => 1,
            SandboxMode::Require => 2,
        }
    }

    /// Parse a config/env value: a recognized token, else `Os`. `None` (absent) is `Os`, matching the
    /// config default (OS confinement on where available).
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

/// The base network stance for a confined `run_command`. `Deserialize`/`from_config` map an unrecognized
/// or absent value to `Deny` (the secure default), so a forward-version or malformed value never widens
/// network access silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStance {
    Deny,
    Allow,
}

impl NetworkStance {
    /// Parse a config/env value: `allow` widens, anything else (including absent) is `Deny`.
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

/// Whether a confined command may open outbound network connections. The resolved policy the `tools`
/// layer consumes (the config resolvers map a [`NetworkStance`] to this). It lives in the kernel beside
/// its sibling stance so neither `config` nor a future `sync` gate has to reach into `tools`.
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
