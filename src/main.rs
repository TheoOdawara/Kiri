mod app;
mod modules;
mod shared;

#[cfg(test)]
mod characterization;

use clap::Parser;

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::modules::sync::application::sync_service::SyncService;
use crate::modules::sync::infrastructure::git_cli::GitCli;
use crate::shared::infra::config::{Cli, CliCommand, Settings, SyncAction, kiri_global_dir};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // A subcommand runs headless (no TTY); only the bare invocation boots the interactive TUI.
    if let Some(CliCommand::Sync { action }) = cli.command {
        return run_sync(action).await;
    }
    let settings = Settings::resolve(cli.path, cli.prompt)?;
    app::wire(settings).await?.run().await
}

/// The headless `kiri sync …` route: build the sync service over the harness home and run the action,
/// printing a one-line summary. Never needs a terminal, so it works over SSH and in scripts.
async fn run_sync(action: SyncAction) -> anyhow::Result<()> {
    let global_dir = kiri_global_dir();
    let config_path = global_dir.join("config.toml");
    let shared_db = global_dir.join("memory").join("shared.db");
    let memory = SqliteSharedMemory::new(shared_db)?;
    memory.init().await?;
    let git = GitCli;
    let service = SyncService::new(&git, global_dir, config_path, &memory);
    let summary = match action {
        SyncAction::Init { url } => service.init(&url).await,
        SyncAction::Push => service.push().await,
        SyncAction::Pull { force } => service.pull(force).await,
        SyncAction::Status => service.status().await,
    }?;
    println!("kiri sync: {summary}");
    Ok(())
}
