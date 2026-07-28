#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod noop;
#[cfg(windows)]
pub mod windows;

use std::path::Path;
use std::sync::Arc;

use crate::modules::tools::application::command_sandbox::CommandSandbox;

/// Select the OS command-sandbox adapter for the current platform. When `enabled` is false
/// (`KIRI_SANDBOX=off`) or no facility is available, the no-op adapter is returned and the
/// path-policy + confirmation layers remain the only guards. macOS uses a Seatbelt profile via
/// `sandbox-exec`; Linux uses a Bubblewrap (`bwrap`) launcher, when it is installed and unprivileged
/// user namespaces actually work (`detect()` probes rather than trusting `PATH`); Windows re-executes
/// this binary as `kiri confined-exec`, which spawns the command under a restricted token — writes only,
/// no network or read confinement there (ADR 0031).
/// Returns the adapter plus, when confinement was wanted but could not be set up for `workspace`, the
/// reason — so the composition root can say it once in the transcript rather than letting every command
/// fail with the same message.
pub fn default_command_sandbox(
    enabled: bool,
    workspace: &Path,
) -> (Arc<dyn CommandSandbox>, Option<String>) {
    let _ = workspace;
    if enabled {
        #[cfg(target_os = "macos")]
        if let Some(adapter) = macos::MacosSeatbelt::detect() {
            return (Arc::new(adapter), None);
        }
        #[cfg(target_os = "linux")]
        if let Some(adapter) = linux::BwrapSandbox::detect() {
            return (Arc::new(adapter), None);
        }
        #[cfg(windows)]
        match windows::WindowsRestrictedToken::detect(workspace) {
            Ok(adapter) => return (Arc::new(adapter), None),
            Err(reason) => return (Arc::new(noop::NoConfinement), Some(reason)),
        }
    }
    (Arc::new(noop::NoConfinement), None)
}
