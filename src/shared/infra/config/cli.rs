use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "kiri",
    about = "Kiri — a provider-agnostic coding-agent harness",
    // From `CARGO_PKG_VERSION`, so `kiri --version` cannot drift from the crate it was built from — the
    // first thing a bug report needs and the one thing the binary could not previously answer.
    version,
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
    /// Internal (Windows): re-execution of this binary as the confinement launcher (ADR 0031). Hidden
    /// because it is not a user-facing command — `run_command` builds this invocation itself, and running
    /// it by hand only spawns a command with less access than the shell already has.
    #[command(hide = true)]
    ConfinedExec {
        /// A directory the confined command may write. Repeated once per root.
        #[arg(long = "rw")]
        rw: Vec<PathBuf>,
        /// The program and its arguments, after `--`.
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<std::ffi::OsString>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_internally_consistent() {
        // clap validates argument combinations in debug asserts, which otherwise fire only when a user
        // actually runs the command — `trailing_var_arg` + `last` on `confined-exec` panicked at runtime
        // and compiled clean. This turns that class into a test failure.
        Cli::command().debug_assert();
    }

    #[test]
    fn confined_exec_collects_repeated_roots_and_the_whole_command() {
        let cli = Cli::parse_from([
            "kiri",
            "confined-exec",
            "--rw",
            r"C:\ws",
            "--rw",
            r"C:\cargo",
            "--",
            "pwsh",
            "-Command",
            "echo hi",
        ]);
        let Some(CliCommand::ConfinedExec { rw, command }) = cli.command else {
            panic!("expected the confined-exec subcommand");
        };
        assert_eq!(rw, [PathBuf::from(r"C:\ws"), PathBuf::from(r"C:\cargo")]);
        // The leading `-Command` must survive as an argument, not be parsed as a launcher flag.
        assert_eq!(command, ["pwsh", "-Command", "echo hi"]);
    }
}
