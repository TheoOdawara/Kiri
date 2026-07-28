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

/// Whether this process was re-executed as the Windows confinement launcher (ADR 0031).
///
/// Peeked from argv rather than read off the parsed `Cli`, because the decision it guards has to happen
/// *before* `Cli::parse`: the launcher must inherit exactly the scrubbed environment its parent handed it,
/// and `load_global_env` would seed `~/.kiri/.env` — API keys included — into a process whose whole job is
/// to spawn an untrusted command that inherits its environment. The subcommand name is matched in the
/// subcommand position only, so a prompt containing the word cannot trigger it.
fn is_confinement_launcher() -> bool {
    cfg!(windows)
        && std::env::args_os()
            .nth(1)
            .is_some_and(|arg| arg == "confined-exec")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if is_confinement_launcher() {
        return run_confinement_launcher(Cli::parse());
    }
    let global_dir = global_dir()?;
    // Seed process env from the trusted `~/.kiri/.env` (never the cwd) before resolving config, so a
    // hostile project repo cannot inject a `.env` to redirect credentials or weaken the sandbox (ADR 0020).
    // This runs before `Cli::parse()` because clap reads `env = "KIRI_PATH"`, so its own diagnostic has to
    // wait for the `Settings` that carries it to the user.
    let env_warning = load_global_env(&global_dir);
    let cli = Cli::parse();
    // Resolve once up front, then dispatch: a subcommand runs headless (no TTY) through the composition
    // root, the bare invocation boots the TUI. `resolve` is TTY-independent but NOT side-effect-free — on a
    // first run it seeds a starter `~/.kiri/config.toml` and hardens `~/.kiri` (0700). That now also
    // applies to `kiri sync`, which is acceptable: sync owns and syncs that very config.
    let mut settings = Settings::resolve(global_dir, cli.path, cli.prompt, cli.instructions)?;
    // The `.env` read precedes resolve, so its warning belongs first in the transcript.
    settings.warnings.splice(0..0, env_warning);
    if let Some(CliCommand::Sync { action }) = cli.command {
        return app::wire_sync(&settings, action).await;
    }
    app::wire(settings).await?.run().await
}

/// Spawn the confined command and exit with *its* status, so the outer `exec::run` reads the real
/// command's exit code rather than the launcher's. A confinement failure is a non-zero exit with the
/// reason on stderr, which `run_command` already surfaces as the command's output.
#[cfg(windows)]
fn run_confinement_launcher(cli: Cli) -> anyhow::Result<()> {
    use crate::modules::tools::infrastructure::confine::windows::restricted;

    let Some(CliCommand::ConfinedExec { rw, command }) = cli.command else {
        return Err(anyhow!("confined-exec requires a command after `--`"));
    };
    let Some((program, args)) = command.split_first() else {
        return Err(anyhow!("confined-exec requires a command after `--`"));
    };
    match restricted::run_confined(&rw, program, args) {
        Ok(code) => std::process::exit(code as i32),
        Err(reason) => Err(anyhow!("{reason}")),
    }
}

/// Never reached: [`is_confinement_launcher`] is `false` off Windows, so the subcommand cannot dispatch
/// there. Present so `main` needs no `cfg` of its own.
#[cfg(not(windows))]
fn run_confinement_launcher(_cli: Cli) -> anyhow::Result<()> {
    Err(anyhow!("confined-exec is a Windows-only internal command"))
}
