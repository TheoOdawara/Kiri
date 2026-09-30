use std::path::PathBuf;

use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::sandbox::NetworkPolicy;

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

    /// Whether this adapter actually enforces OS-level confinement on the current platform. `false`
    /// for the no-op adapter (unsupported platform or `KIRI_SANDBOX=off`); `run_command` consults
    /// this to honor `KIRI_SANDBOX=require`.
    fn supports_confinement(&self) -> bool;
}
