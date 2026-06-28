#[cfg(target_os = "macos")]
pub mod macos;
pub mod noop;

use std::sync::Arc;

use crate::modules::tools::application::command_sandbox::CommandSandbox;

/// Select the OS command-sandbox adapter for the current platform. When `enabled` is false
/// (`KIRI_SANDBOX=off`) or no facility is available, the no-op adapter is returned and the
/// path-policy + confirmation layers remain the only guards. macOS (the v1 target) uses a Seatbelt
/// profile via `sandbox-exec`; non-macOS platforms resolve to the no-op adapter — OS confinement
/// there is future work, gated behind a real Windows/Linux release (`KIRI_SANDBOX=require` refuses
/// to run unconfined in the meantime).
pub fn default_command_sandbox(enabled: bool) -> Arc<dyn CommandSandbox> {
    if enabled {
        #[cfg(target_os = "macos")]
        if let Some(adapter) = macos::MacosSeatbelt::detect() {
            return Arc::new(adapter);
        }
    }
    Arc::new(noop::NoConfinement)
}
