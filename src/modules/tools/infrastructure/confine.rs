#[cfg(target_os = "macos")]
pub mod macos;
pub mod noop;

use std::sync::Arc;

use crate::modules::tools::application::command_sandbox::CommandSandbox;

/// Select the OS command-sandbox adapter for the current platform. When `enabled` is false
/// (`KIRI_SANDBOX=off`) or no facility is available, the no-op adapter is returned and the
/// path-policy + confirmation layers remain the only guards. macOS uses a Seatbelt profile via
/// `sandbox-exec`; the Linux Landlock adapter is tracked as follow-up (it needs a Linux host to
/// verify), so Linux currently resolves to the no-op adapter.
pub fn default_command_sandbox(enabled: bool) -> Arc<dyn CommandSandbox> {
    if enabled {
        #[cfg(target_os = "macos")]
        if let Some(adapter) = macos::MacosSeatbelt::detect() {
            return Arc::new(adapter);
        }
    }
    Arc::new(noop::NoConfinement)
}
