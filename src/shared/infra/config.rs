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
pub use writers::{persist_active_model, persist_active_provider, persist_effort, upsert_provider};

pub(crate) use raw::validate_config_str;
pub(crate) use writers::ensure_private_dir;

#[cfg(test)]
mod tests {
    // `shared` is the leaf every module depends on; this foundation file must never reach *down* into a
    // module. Guard the invariant structurally so a future edit cannot silently re-introduce the edge.
    #[test]
    fn config_has_no_module_imports() {
        let source = include_str!("config.rs");
        // Build the needle by concatenation so this guard's own literal does not self-match the file.
        let needle = concat!("use crate", "::modules::");
        assert!(
            !source.contains(needle),
            "shared/infra/config.rs must not import from any module"
        );
    }
}
