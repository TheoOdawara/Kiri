mod app;
mod modules;
mod shared;

#[cfg(test)]
mod architecture_guards;

use std::path::PathBuf;

use anyhow::anyhow;
use clap::Parser;

use crate::shared::infra::config::{Cli, CliCommand, Settings, load_global_env};
use crate::shared::infra::home;

/// The harness home, resolved once and injected into everything that needs it (ADR 0015). A machine with
/// no resolvable home fails fast and names the variables checked: the former fallback took `~/.kiri`
/// literally, which would silently create a directory named `~` in the cwd and scatter credentials,
/// sessions, and config into whatever directory the user happened to launch from.
fn global_dir() -> anyhow::Result<PathBuf> {
    home::home_dir()
        .map(|home| home.join(".kiri"))
        .ok_or_else(|| {
            anyhow!("cannot resolve a home directory: set HOME, USERPROFILE, or HOMEDRIVE+HOMEPATH")
        })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let global_dir = global_dir()?;
    // Seed process env from the trusted `~/.kiri/.env` (never the cwd) before resolving config, so a
    // hostile project repo cannot inject a `.env` to redirect credentials or weaken the sandbox (ADR 0020).
    load_global_env(&global_dir);
    let cli = Cli::parse();
    // Resolve once up front, then dispatch: a subcommand runs headless (no TTY) through the composition
    // root, the bare invocation boots the TUI. `resolve` is TTY-independent but NOT side-effect-free — on a
    // first run it seeds a starter `~/.kiri/config.toml` and hardens `~/.kiri` (0700). That now also
    // applies to `kiri sync`, which is acceptable: sync owns and syncs that very config.
    let settings = Settings::resolve(global_dir, cli.path, cli.prompt, cli.instructions)?;
    if let Some(CliCommand::Sync { action }) = cli.command {
        return app::wire_sync(&settings, action).await;
    }
    app::wire(settings).await?.run().await
}
