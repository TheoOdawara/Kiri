#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod noop;

use std::sync::Arc;

use crate::modules::tools::application::command_sandbox::CommandSandbox;

/// Select the OS command-sandbox adapter for the current platform. When `enabled` is false
/// (`KIRI_SANDBOX=off`) or no facility is available, the no-op adapter is returned and the
/// path-policy + confirmation layers remain the only guards. macOS uses a Seatbelt profile via
/// `sandbox-exec`; Linux uses a Bubblewrap (`bwrap`) launcher, when it is installed and unprivileged
/// user namespaces actually work (`detect()` probes rather than trusting `PATH`); Windows resolves to
/// the no-op adapter — OS confinement there is tracked follow-up (`KIRI_SANDBOX=require` refuses to
/// run unconfined in the meantime).
pub fn default_command_sandbox(enabled: bool) -> Arc<dyn CommandSandbox> {
    if enabled {
        #[cfg(target_os = "macos")]
        if let Some(adapter) = macos::MacosSeatbelt::detect() {
            return Arc::new(adapter);
        }
        #[cfg(target_os = "linux")]
        if let Some(adapter) = linux::BwrapSandbox::detect() {
            return Arc::new(adapter);
        }
    }
    Arc::new(noop::NoConfinement)
}
