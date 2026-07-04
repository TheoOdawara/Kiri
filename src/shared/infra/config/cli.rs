use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "kiri",
    about = "Kiri — a provider-agnostic coding-agent harness",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CliCommand>,
    /// Optional first message; the chat then continues interactively
    pub prompt: Option<String>,
    /// Sandbox root for file tools (also via KIRI_PATH). Defaults to the current directory.
    #[arg(long, env = "KIRI_PATH")]
    pub path: Option<PathBuf>,
    /// Override the auto-discovered instructions file (KIRI.md / AGENTS.md / CLAUDE.md).
    #[arg(long)]
    pub instructions: Option<PathBuf>,
}

/// The top-level subcommands. Absent → the interactive TUI; present → a headless command that runs
/// without a TTY (so `kiri sync …` works over SSH / in scripts).
#[derive(clap::Subcommand)]
pub enum CliCommand {
    /// Sync the portable profile (non-secret config + shared memory) with a private git repo.
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },
}

/// The `kiri sync` actions.
#[derive(clap::Subcommand)]
pub enum SyncAction {
    /// Point sync at a private repo and set up the local work-tree.
    Init {
        /// The git remote URL (SSH or HTTPS) of your private profile repo.
        url: String,
    },
    /// Export the profile, commit, and push to the remote.
    Push,
    /// Pull and merge the profile (memory last-write-wins; config under a trust check).
    Pull {
        /// Apply an incoming config even if it changes a provider base_url or weakens the sandbox.
        #[arg(long)]
        force: bool,
    },
    /// Show the sync work-tree's git status.
    Status,
}
