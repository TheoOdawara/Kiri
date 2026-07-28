pub mod restricted;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};
use crate::shared::kernel::error::AgentError;

/// Windows OS-confinement adapter (ADR 0031). Confines **writes** to the policy's roots with a restricted
/// token carrying the RESTRICTED restricting SID, so an access check succeeds only where an ACE grants
/// that SID — which `restricted::run_confined` adds to those roots.
///
/// The wrapping mirrors macOS's `sandbox-exec` and Linux's `bwrap`, except Windows ships no such launcher:
/// `CreateProcessAsUser` *is* the spawn, and the `CommandSandbox` port decorates rather than spawns so the
/// single spawn site in `exec::run` keeps its timeout and tree-kill. The launcher is therefore this same
/// binary re-executed as `kiri confined-exec`, which does the Win32 work and waits. The existing Job Object
/// around the outer spawn already kills the grandchild, so a timeout still reaps the real command.
///
/// **Reads are not confined**, unlike Seatbelt and bwrap: the credential-directory guarantee those two give
/// does not hold here, and `run_command`'s sensitive-path checks remain the only barrier. Network denial is
/// likewise not enforced — see `supports_confinement`'s note and ADR 0031.
#[derive(Debug)]
pub struct WindowsRestrictedToken {
    launcher: PathBuf,
}

impl WindowsRestrictedToken {
    /// `Ok` when this binary can act as the launcher *and* `workspace` accepts the write grant the
    /// mechanism needs; `Err` carries the reason so the composition root can say it once at boot. The
    /// grant is probed rather than assumed because it needs `WRITE_DAC`, which only the directory's real
    /// owner holds — a workspace on a secondary drive typically inherits its ACL from a root owned by
    /// Administrators and reaches the user through a group grant that carries no `WRITE_DAC` (ADR 0031).
    pub fn detect(workspace: &Path) -> Result<Self, String> {
        let launcher = std::env::current_exe().map_err(|error| {
            format!("cannot resolve this executable to use as the launcher: {error}")
        })?;
        if !launcher.is_file() {
            return Err(format!("{} is not a file", launcher.display()));
        }
        restricted::can_confine(workspace)?;
        Ok(Self { launcher })
    }

    /// Test-only: under `cargo test` this process is the libtest harness, not the CLI, so a self-exec
    /// would fail to parse `confined-exec` and the command would never run — an escape test would then
    /// pass because nothing happened at all. The integration tests point at the built `kiri.exe` instead.
    #[cfg(test)]
    pub(crate) fn with_launcher(launcher: PathBuf, workspace: &Path) -> Result<Self, String> {
        if !launcher.is_file() {
            return Err(format!("{} is not a file", launcher.display()));
        }
        restricted::can_confine(workspace)?;
        Ok(Self { launcher })
    }
}

impl CommandSandbox for WindowsRestrictedToken {
    fn confine(
        &self,
        cmd: tokio::process::Command,
        policy: &SandboxPolicy,
    ) -> Result<tokio::process::Command, AgentError> {
        // Fail closed, the same way the macOS adapter re-checks `sandbox-exec`: a launcher that vanished
        // since detection must refuse, never silently spawn the command unconfined.
        if !self.launcher.is_file() {
            return Err(AgentError::Sandbox(format!(
                "{} is unavailable; cannot confine the command",
                self.launcher.display()
            )));
        }
        let std = cmd.as_std();
        let program = std.get_program().to_owned();
        let args: Vec<OsString> = std.get_args().map(OsStr::to_owned).collect();
        let cwd = std.get_current_dir().map(Path::to_owned);
        let envs: Vec<(OsString, Option<OsString>)> = std
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
            .collect();

        let mut wrapped = tokio::process::Command::new(&self.launcher);
        wrapped.arg("confined-exec");
        for root in writable_roots(policy) {
            wrapped.arg("--rw").arg(root);
        }
        // `--` so a command starting with a hyphen is not eaten as a launcher flag.
        wrapped.arg("--").arg(program).args(&args);
        if let Some(dir) = cwd {
            wrapped.current_dir(dir);
        }
        // Same reasoning as the macOS adapter: `get_envs()` reports only explicit overrides and cannot
        // report that `env_clear()` was called, so the rebuilt command must clear before replaying or a
        // scrubbed caller's environment (issues #25/#49) would fully re-inherit — and here it would then
        // be inherited a second time by the confined grandchild.
        wrapped.env_clear();
        for (key, value) in envs {
            match value {
                Some(value) => wrapped.env(key, value),
                None => wrapped.env_remove(key),
            };
        }
        Ok(wrapped)
    }

    fn supports_confinement(&self) -> bool {
        // True for the guarantee this adapter actually makes — writes are confined. It deliberately does
        // not claim the network denial `NetworkPolicy::Deny` asks for: enforcing that needs a per-process
        // firewall rule, which needs elevation (ADR 0031). Overstating it here would let
        // `KIRI_SANDBOX=require` report a guarantee the OS is not providing.
        true
    }
}

/// The workspace root, the configured toolchain directories, and the temp directory. `extra_ro` is not
/// consulted: reads are not confined on this platform, so a read-only grant would be a no-op that reads
/// like a guarantee.
///
/// `%TEMP%` is added here rather than in the shared `DEFAULT_RW_DIRS` because the need is
/// Windows-specific and not expressible as a `~` path: the MSVC linker writes `lnk{…}.tmp` there, so
/// without it `cargo build` fails with `LNK1104: cannot open file`. Granting it is cheap — the temp
/// directory is a low-value target that the OS already treats as scratch — and it is the same accomodation
/// the Codex sandbox makes for "special temp directories".
fn writable_roots(policy: &SandboxPolicy) -> Vec<PathBuf> {
    std::iter::once(policy.root.clone())
        .chain(policy.extra_rw.iter().cloned())
        .chain(std::iter::once(std::env::temp_dir()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::sandbox::NetworkPolicy;
    use tempfile::TempDir;

    /// A temp dir is owned by the current user, so it accepts the write grant `detect` probes for.
    fn adapter(workspace: &TempDir) -> WindowsRestrictedToken {
        WindowsRestrictedToken::detect(workspace.path())
            .expect("a user-owned workspace is confinable")
    }

    fn policy(root: &Path) -> SandboxPolicy {
        SandboxPolicy {
            root: root.to_path_buf(),
            network: NetworkPolicy::Deny,
            extra_ro: vec![PathBuf::from("C:\\ro-never-granted")],
            extra_rw: vec![PathBuf::from("C:\\rw")],
        }
    }

    #[test]
    fn writable_roots_are_the_workspace_the_extras_and_temp() {
        let roots = writable_roots(&policy(Path::new("C:\\ws")));
        assert_eq!(
            roots,
            [
                PathBuf::from("C:\\ws"),
                PathBuf::from("C:\\rw"),
                std::env::temp_dir()
            ],
            "temp is required: the MSVC linker writes lnk{{…}}.tmp there (LNK1104 without it)"
        );
        // `extra_ro` must never appear: reads are not confined here, so granting it write would widen
        // the sandbox while reading like a read guarantee.
        assert!(!roots.contains(&PathBuf::from("C:\\ro-never-granted")));
    }

    #[test]
    fn confine_wraps_the_command_in_the_launcher_and_keeps_program_and_args() {
        let workspace = TempDir::new().unwrap();
        let adapter = adapter(&workspace);
        let mut original = tokio::process::Command::new("pwsh");
        original.args(["-Command", "echo hi"]);
        original.current_dir("C:\\ws");
        let wrapped = adapter
            .confine(original, &policy(Path::new("C:\\ws")))
            .expect("the launcher exists");
        let std = wrapped.as_std();
        let args: Vec<String> = std
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(std.get_program(), std::env::current_exe().unwrap());
        let temp = std::env::temp_dir().to_string_lossy().into_owned();
        assert_eq!(
            args,
            [
                "confined-exec",
                "--rw",
                "C:\\ws",
                "--rw",
                "C:\\rw",
                "--rw",
                &temp,
                "--",
                "pwsh",
                "-Command",
                "echo hi"
            ]
        );
        // The cwd survives the rebuild, or the command would run somewhere else entirely.
        assert_eq!(std.get_current_dir(), Some(Path::new("C:\\ws")));
    }

    #[test]
    fn confine_clears_the_environment_before_replaying_the_scrubbed_overrides() {
        let workspace = TempDir::new().unwrap();
        let adapter = adapter(&workspace);
        let mut original = tokio::process::Command::new("pwsh");
        original.env_clear();
        original.env("PATH", "C:\\keep");
        let wrapped = adapter
            .confine(original, &policy(Path::new("C:\\ws")))
            .expect("the launcher exists");
        let envs: Vec<(String, Option<String>)> = wrapped
            .as_std()
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(envs, [("PATH".to_string(), Some("C:\\keep".to_string()))]);
    }

    #[test]
    fn detect_refuses_a_workspace_that_cannot_take_the_grant() {
        // A path that does not exist stands in for the real case — a workspace whose ACL the user cannot
        // write. Either way `detect` must report *why* instead of returning an adapter that would then
        // fail on every single command.
        let missing = std::path::Path::new(r"C:\kiri-no-such-workspace-9f3a");
        let error = WindowsRestrictedToken::detect(missing)
            .expect_err("an unwritable workspace cannot be confined");
        assert!(error.contains("kiri-no-such-workspace"), "got: {error}");
    }
}
