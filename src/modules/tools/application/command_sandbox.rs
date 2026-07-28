use std::path::PathBuf;

use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::sandbox::NetworkPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxGuarantees {
    pub filesystem_read: bool,
    pub filesystem_write: bool,
    pub network: bool,
    pub process_and_ipc: bool,
    pub host_interop: bool,
    pub protected_secrets: bool,
}

impl SandboxGuarantees {
    pub const NONE: Self = Self {
        filesystem_read: false,
        filesystem_write: false,
        network: false,
        process_and_ipc: false,
        host_interop: false,
        protected_secrets: false,
    };

    pub const MACOS: Self = Self {
        filesystem_read: false,
        filesystem_write: true,
        network: true,
        process_and_ipc: false,
        host_interop: false,
        protected_secrets: true,
    };

    pub const BWRAP: Self = Self {
        filesystem_read: true,
        filesystem_write: true,
        network: true,
        process_and_ipc: true,
        host_interop: true,
        protected_secrets: true,
    };

    pub fn satisfies(self, policy: &SandboxPolicy) -> bool {
        self.filesystem_write
            && self.protected_secrets
            && (policy.network != NetworkPolicy::Deny || self.network)
    }
}

/// The OS-confinement policy for a single command: the workspace root it may write under, the network
/// stance, and any extra paths a legitimate operation needs (toolchain dirs from config, or an
/// approved out-of-root target for that one call). Pure data — no I/O — so it lives in the
/// application layer alongside the port that consumes it.
#[derive(Debug, Clone)]
// The macOS Seatbelt and Linux bwrap adapters are the only consumers of these fields; Windows
// resolves to the no-op adapter (tracked follow-up), so they read as dead there. The lint stays
// active on macOS/Linux to catch a field that becomes genuinely unused on either target.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub struct SandboxPolicy {
    pub root: PathBuf,
    pub command_home: PathBuf,
    pub workspace_access: WorkspaceAccess,
    pub network: NetworkPolicy,
    pub extra_ro: Vec<PathBuf>,
    pub extra_rw: Vec<PathBuf>,
}

/// Port: confine a child process to the workspace at the OS level before it is spawned. The adapter
/// *decorates* an already-built `tokio::process::Command` — it never spawns — so the single spawn
/// site in `exec::run` (with its timeout, `kill_on_drop`, and piped stdio) is preserved. Implemented
/// per platform in `tools::infrastructure::confine`; a no-op adapter covers platforms without an OS
/// facility and the `KIRI_SANDBOX=off` opt-out.
pub trait CommandSandbox: Send + Sync + std::fmt::Debug {
    /// Rewrite `cmd` so the child runs confined under `policy`. Returns the decorated command, or an
    /// `AgentError::Sandbox` if confinement could not be set up.
    fn confine(
        &self,
        cmd: tokio::process::Command,
        policy: &SandboxPolicy,
    ) -> Result<tokio::process::Command, AgentError>;

    fn guarantees(&self) -> SandboxGuarantees;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_guarantees_never_satisfy_required_confinement() {
        let policy = SandboxPolicy {
            root: PathBuf::from("/workspace"),
            command_home: PathBuf::from("/kiri/home"),
            workspace_access: WorkspaceAccess::ReadOnly,
            network: NetworkPolicy::Deny,
            extra_ro: Vec::new(),
            extra_rw: Vec::new(),
        };

        assert!(!SandboxGuarantees::NONE.satisfies(&policy));
    }

    #[test]
    fn bwrap_guarantees_satisfy_read_only_offline_plan_execution() {
        let policy = SandboxPolicy {
            root: PathBuf::from("/workspace"),
            command_home: PathBuf::from("/kiri/home"),
            workspace_access: WorkspaceAccess::ReadOnly,
            network: NetworkPolicy::Deny,
            extra_ro: Vec::new(),
            extra_rw: Vec::new(),
        };

        assert!(SandboxGuarantees::BWRAP.satisfies(&policy));
    }
}
