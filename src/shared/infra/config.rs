//! Layered TOML configuration. A re-exporting facade over the `config/` submodules: the public surface
//! (`Settings`, the `Cli`, the live-write helpers) is re-exported here so every external `config::X`
//! call site is unchanged. Cross-submodule visibility is `pub(super)`.

mod cli;
mod defaults;
mod raw;
mod resolve;
mod settings;
mod system_prompt;
mod writers;

pub use cli::{Cli, CliCommand, SyncAction};
pub use settings::Settings;
pub use system_prompt::render_system_prompt;
pub use writers::{persist_active_model, persist_active_provider, persist_effort, upsert_provider};

pub(crate) use raw::validate_config_str;
pub(crate) use writers::ensure_private_dir;

#[cfg(test)]
mod tests {
    // `shared` is the leaf every module depends on; this foundation file must never reach *down* into a
    // module. Guard the invariant structurally so a future edit cannot silently re-introduce the edge.
    #[test]
    fn config_has_no_module_imports() {
        // Scan the facade AND every submodule: the imports live in the submodules after the split, so a
        // facade-only scan would pass tautologically and let the leaf-module invariant rot silently.
        let sources = [
            include_str!("config.rs"),
            include_str!("config/system_prompt.rs"),
            include_str!("config/defaults.rs"),
            include_str!("config/raw.rs"),
            include_str!("config/resolve.rs"),
            include_str!("config/writers.rs"),
            include_str!("config/cli.rs"),
            include_str!("config/settings.rs"),
        ];
        // Build the needle by concatenation so this guard's own literal does not self-match.
        let needle = concat!("use crate", "::modules::");
        for source in sources {
            assert!(
                !source.contains(needle),
                "shared/infra/config (incl. submodules) must not import from any module"
            );
        }
    }
}
