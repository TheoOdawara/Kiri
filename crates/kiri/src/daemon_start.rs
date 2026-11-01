use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow};
use kiri_core::protocol::{self, Connection, Endpoint};
use tokio::process::Command;
use tokio::time::Instant;

const CONNECT_DEADLINE: Duration = Duration::from_secs(5);
const CONNECT_RETRY: Duration = Duration::from_millis(50);
const RESPAWN_AFTER: Duration = Duration::from_millis(500);

pub async fn reach(endpoint: &Endpoint, executable: &Path) -> Result<Connection> {
    let deadline = Instant::now() + CONNECT_DEADLINE;
    let mut last_spawn: Option<Instant> = None;
    loop {
        if let Ok(Ok(connection)) =
            tokio::time::timeout_at(deadline, protocol::connect(endpoint)).await
        {
            return Ok(connection);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "Could not connect to the Kiri daemon within 5 seconds. See {}.",
                endpoint.log_path().display()
            ));
        }
        if last_spawn.is_none_or(|spawned_at| spawned_at.elapsed() >= RESPAWN_AFTER) {
            spawn_daemon(executable, endpoint.home())?;
            last_spawn = Some(Instant::now());
        }
        tokio::time::sleep(CONNECT_RETRY).await;
    }
}

fn spawn_daemon(executable: &Path, home: &Path) -> Result<()> {
    let mut command = Command::new(executable);
    command
        .arg("daemon")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach_from_terminal(&mut command);
    command
        .spawn()
        .map_err(|cause| anyhow!("Could not start the Kiri daemon: {cause}"))?;
    Ok(())
}

#[cfg(unix)]
fn detach_from_terminal(command: &mut Command) {
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach_from_terminal(command: &mut Command) {
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_executable_the_os_refuses_to_spawn_is_reported_with_its_cause() {
        let missing = std::env::temp_dir().join("kiri-this-executable-does-not-exist");

        let error = spawn_daemon(&missing, &std::env::temp_dir()).unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("Could not start the Kiri daemon: "),
            "{error}"
        );
    }
}
