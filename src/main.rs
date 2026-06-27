mod app;
mod modules;
mod shared;

#[cfg(test)]
mod characterization;

use clap::Parser;

use crate::shared::infra::config::{Cli, CliCommand, Settings};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Settings::resolve is side-effect-light and TTY-independent, so resolve once up front and dispatch:
    // a subcommand runs headless (no TTY) through the composition root, the bare invocation boots the TUI.
    let settings = Settings::resolve(cli.path, cli.prompt)?;
    if let Some(CliCommand::Sync { action }) = cli.command {
        return app::wire_sync(&settings, action).await;
    }
    app::wire(settings).await?.run().await
}
