mod app;
mod modules;
mod shared;

#[cfg(test)]
mod architecture_guards;
#[cfg(test)]
mod characterization;

use clap::Parser;

use crate::shared::infra::config::{Cli, CliCommand, Settings};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Resolve once up front, then dispatch: a subcommand runs headless (no TTY) through the composition
    // root, the bare invocation boots the TUI. `resolve` is TTY-independent but NOT side-effect-free — on a
    // first run it seeds a starter `~/.kiri/config.toml` and hardens `~/.kiri` (0700). That now also
    // applies to `kiri sync`, which is acceptable: sync owns and syncs that very config.
    let settings = Settings::resolve(cli.path, cli.prompt)?;
    if let Some(CliCommand::Sync { action }) = cli.command {
        return app::wire_sync(&settings, action).await;
    }
    app::wire(settings).await?.run().await
}
