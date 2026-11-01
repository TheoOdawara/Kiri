mod daemon_start;

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use kiri_core::protocol::Endpoint;

#[derive(Parser)]
#[command(name = "kiri", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Hosts the sessions of this user. Clients start it on their own.
    Daemon,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime =
        tokio::runtime::Runtime::new().expect("the OS provides threads and an event queue");
    let exit_code = runtime.block_on(async {
        match cli.command {
            Some(Command::Daemon) => kiri_core::daemon::run().await,
            None => match run_client().await {
                Ok(()) => ExitCode::SUCCESS,
                Err(cause) => {
                    eprintln!("{cause}");
                    ExitCode::FAILURE
                }
            },
        }
    });
    runtime.shutdown_background();
    exit_code
}

async fn run_client() -> Result<()> {
    let endpoint = Endpoint::resolve()?;
    let executable = std::env::current_exe()
        .map_err(|cause| anyhow!("Could not start the Kiri daemon: {cause}"))?;
    let connection = daemon_start::reach(&endpoint, &executable).await?;
    kiri_tui::line_client::run(connection).await
}
