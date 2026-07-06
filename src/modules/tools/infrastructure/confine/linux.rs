use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};
use crate::modules::tools::infrastructure::secret_paths::{
    HARNESS_PRIVATE_DIR, HOME_SECRET_FILES, SECRET_DIRS,
};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::sandbox::NetworkPolicy;

const BWRAP: &str = "bwrap";

/// Linux OS-confinement adapter. Wraps the child in `bwrap <flags> -- <program> <args…>`, mirroring the
/// macOS Seatbelt adapter's argv-transform shape. A system binary — no FFI, no crate — so the crate-wide
/// `unsafe_code = "forbid"` lint is untouched. `--ro-bind / /` re-mounts the whole filesystem read-only
/// (the permissive base matching Seatbelt's `(allow default)`), then targeted `--bind`s re-open the
/// workspace and configured extras for writing, and `--tmpfs`/`--ro-bind /dev/null` shadow the
/// single-sourced credential set so a confined `run_command` cannot read it back to the model.
///
/// Landlock (the more modern, no-external-binary approach) is deferred: it is deny-by-default
/// allow-list, which cannot express "read everything except `~/.ssh`" without enumerating the rest of
/// the filesystem, and confining only the spawned child needs either `pre_exec` (`unsafe`) or its own
/// launcher binary — strictly more work than bwrap for a worse semantic match, and needs kernel ≥6.7 for
/// its network-deny ruleset. Tracked in ADR 0009/0018 as the follow-up if bwrap's own gaps (see below)
/// prove insufficient.
#[derive(Debug)]
pub struct BwrapSandbox;

impl BwrapSandbox {
    /// Available only when `bwrap` actually works, not merely when it is on `PATH`: Ubuntu 24.04+ ships
    /// `bwrap` but can block unprivileged user namespaces via AppArmor
    /// (`kernel.apparmor_restrict_unprivileged_userns=1`), which makes an installed `bwrap` fail at
    /// runtime. Probing with a real minimal sandbox invocation is the only way to tell the two states
    /// apart; `confine()` re-probes fail-closed for the same reason the macOS adapter re-checks
    /// `sandbox-exec`'s presence (the facility can vanish/break between detection and use).
    pub fn detect() -> Option<Self> {
        probe().then_some(Self)
    }
}

/// Run bwrap against `/bin/true` inside the same minimal jail shape `confine()` builds (whole-FS
/// read-only bind, network unshared, `/dev` present) and require exit 0. A cheap, synchronous check —
/// called once at startup (`detect`) and again on every `confine()` call (fail-closed).
fn probe() -> bool {
    std::process::Command::new(BWRAP)
        .args([
            "--ro-bind",
            "/",
            "/",
            "--unshare-net",
            "--dev",
            "/dev",
            "--",
            "/bin/true",
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

impl CommandSandbox for BwrapSandbox {
    fn confine(
        &self,
        cmd: tokio::process::Command,
        policy: &SandboxPolicy,
    ) -> Result<tokio::process::Command, AgentError> {
        // Fail closed: if bwrap stopped working since detection (AppArmor policy change, binary
        // removed), refuse rather than spawn unconfined.
        if !probe() {
            return Err(AgentError::Sandbox(
                "bwrap is unavailable; cannot confine the command".to_string(),
            ));
        }
        // Read the built command's program/args/cwd/env back out (stdio is set later, at the single
        // spawn site), then rebuild it behind the bwrap wrapper preserving all of them.
        let std = cmd.as_std();
        let program = std.get_program().to_owned();
        let args: Vec<OsString> = std.get_args().map(std::ffi::OsStr::to_owned).collect();
        let cwd = std.get_current_dir().map(Path::to_owned);
        let envs: Vec<(OsString, Option<OsString>)> = std
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(std::ffi::OsStr::to_owned)))
            .collect();

        let mut wrapped = tokio::process::Command::new(BWRAP);
        wrapped.args(build_args(policy, cwd.as_deref()));
        wrapped.arg("--").arg(program).args(&args);
        // `get_envs()` only reports explicit overrides made via `env`/`env_remove`/`env_clear` on the
        // ORIGINAL `cmd` — it cannot see whether `env_clear()` itself was called, so replaying it onto a
        // fresh `Command` here is not enough on its own: without also clearing `wrapped`, a scrubbed
        // caller's env (see `exec::run_shell`, issues #25/#49) would still fully inherit into this
        // rebuilt process, silently re-leaking every credential the caller just cleared.
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
        true
    }
}

/// Build the bwrap flag list from the policy. bwrap applies binds in argument order: a later bind
/// shadows an earlier one at the same or a nested path, so the shape mirrors Seatbelt's last-match-wins
/// profile: whole-FS read-only base, then write-allows for the workspace/extras, then credential
/// shadows, then explicit read re-allows last so they can punch a hole through the credential shadow.
fn build_args(policy: &SandboxPolicy, cwd: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();

    // Permissive base: the whole filesystem, read-only, matching Seatbelt's `(allow default)` posture -
    // the path-policy and confirmation layers are the primary guard; this OS layer only adds what they
    // cannot enforce.
    push_self_bind(&mut args, "--ro-bind", Path::new("/"));
    push_dest_only(&mut args, "--dev", Path::new("/dev"));
    push_dest_only(&mut args, "--proc", Path::new("/proc"));
    push_dest_only(&mut args, "--tmpfs", &std::env::temp_dir());

    // Writes: re-open the workspace root and the configured/per-call extras (bind, not ro-bind, so they
    // are writable over the read-only base).
    push_self_bind(&mut args, "--bind", &policy.root);
    for dir in &policy.extra_rw {
        push_self_bind(&mut args, "--bind", dir);
    }

    // Credential set under home, shadowed AFTER the write-allows above so they win even when the
    // workspace root (or an `extra_rw`) is a home ancestor (e.g. after `/cd ~`) - mirrors the macOS
    // adapter's last-match-wins credential denies. A directory is shadowed with an empty `--tmpfs`
    // (looks empty but still traversable, so the parent directory listing does not break); a single
    // file is shadowed with `--ro-bind /dev/null` (reads as empty, cannot be written).
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for dir in SECRET_DIRS {
            push_dest_only(&mut args, "--tmpfs", &home.join(dir));
        }
        push_dest_only(&mut args, "--tmpfs", &home.join(HARNESS_PRIVATE_DIR));
        for file in HOME_SECRET_FILES {
            push_shadow_file(&mut args, &home.join(file));
        }
    }

    // Re-allow any explicitly configured read paths (KIRI_SANDBOX_RO_PATHS), so the user can punch a
    // read-hole through the credential shadows above. Emitted last so it wins (bwrap applies binds in
    // argument order).
    for dir in &policy.extra_ro {
        push_self_bind(&mut args, "--ro-bind", dir);
    }

    if policy.network == NetworkPolicy::Deny {
        args.push(OsString::from("--unshare-net"));
    }
    args.push(OsString::from("--die-with-parent"));
    if let Some(dir) = cwd {
        push_dest_only(&mut args, "--chdir", dir);
    }
    args
}

/// `--bind`/`--ro-bind SRC DEST` where the destination is the same path as the source: the shape every
/// bind in this adapter uses, since the base already maps the whole filesystem 1:1.
fn push_self_bind(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    let resolved = canon(path);
    args.push(OsString::from(flag));
    args.push(resolved.clone().into_os_string());
    args.push(resolved.into_os_string());
}

/// A bwrap flag that takes a single destination path with no source (`--dev`, `--proc`, `--tmpfs`,
/// `--chdir`).
fn push_dest_only(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    args.push(OsString::from(flag));
    args.push(path.to_owned().into_os_string());
}

/// Shadow a single file with an empty, read-only one via `--ro-bind /dev/null DEST` - bwrap's idiom for
/// hiding one file without affecting its siblings (unlike `--tmpfs`, which replaces a whole directory).
fn push_shadow_file(args: &mut Vec<OsString>, target: &Path) {
    args.push(OsString::from("--ro-bind"));
    args.push(OsString::from("/dev/null"));
    args.push(target.to_owned().into_os_string());
}

/// Canonicalize for a bind-mount source, falling back to the input when the path does not yet exist
/// (e.g. a not-yet-created extra dir) so a bind is still emitted.
fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(network: NetworkPolicy) -> SandboxPolicy {
        SandboxPolicy {
            root: PathBuf::from("/tmp/kiri-ws"),
            network,
            extra_ro: Vec::new(),
            extra_rw: vec![PathBuf::from("/tmp/kiri-extra")],
        }
    }

    fn args_to_strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn args_unshare_net_when_denied() {
        let args = args_to_strings(&build_args(&policy(NetworkPolicy::Deny), None));
        assert!(args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn args_allow_network_when_permitted() {
        let args = args_to_strings(&build_args(&policy(NetworkPolicy::Allow), None));
        assert!(!args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn args_bind_the_workspace_root_and_extras_writable() {
        let args = args_to_strings(&build_args(&policy(NetworkPolicy::Deny), None));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--bind" && w[1].contains("kiri-ws"))
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--bind" && w[1].contains("kiri-extra"))
        );
    }

    #[test]
    fn args_shadow_credential_dirs_after_write_allows() {
        let Some(home) = std::env::var_os("HOME") else {
            return; // headless/CI edge: no per-user home whose credential shadows exist
        };
        let home = PathBuf::from(home);
        let args = build_args(&policy(NetworkPolicy::Deny), None);
        let strings = args_to_strings(&args);
        let root_bind_at = strings
            .iter()
            .position(|a| a.contains("kiri-ws"))
            .expect("root bind present");
        let ssh_shadow_at = strings
            .iter()
            .position(|a| a == home.join(".ssh").to_string_lossy().as_ref())
            .expect("~/.ssh shadow present");
        assert!(
            ssh_shadow_at > root_bind_at,
            "credential shadows must come after the write-allows so they win (argument-order wins)"
        );
        assert!(strings.iter().any(|a| a.contains(".kiri")));
    }

    #[test]
    fn args_reallow_configured_read_paths_last() {
        let mut p = policy(NetworkPolicy::Deny);
        p.extra_ro.push(PathBuf::from("/tmp"));
        let args = build_args(&p, None);
        let strings = args_to_strings(&args);
        let last_ro_bind = strings
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == "--ro-bind")
            .next_back()
            .map(|(i, _)| i)
            .expect("at least one --ro-bind present");
        // The last --ro-bind flag emitted must be the explicit extra_ro re-allow, not the base `/`.
        assert_ne!(strings[last_ro_bind + 1], "/");
    }

    #[test]
    fn args_include_chdir_when_cwd_given() {
        let dir = std::env::temp_dir();
        let args = args_to_strings(&build_args(&policy(NetworkPolicy::Deny), Some(&dir)));
        assert!(args.contains(&"--chdir".to_string()));
    }

    #[tokio::test]
    async fn confine_wraps_the_command_in_bwrap() {
        if BwrapSandbox::detect().is_none() {
            return; // bwrap unavailable/non-functional on this host
        }
        let adapter = BwrapSandbox;
        let mut inner = tokio::process::Command::new("/bin/echo");
        inner.arg("hi");
        let wrapped = adapter
            .confine(inner, &policy(NetworkPolicy::Deny))
            .unwrap();
        let std = wrapped.as_std();
        assert_eq!(std.get_program(), BWRAP);
        let args: Vec<_> = std
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.last().unwrap(), "hi");
        assert_eq!(args[args.len() - 2], "/bin/echo");
    }

    #[tokio::test]
    async fn confine_does_not_leak_the_full_parent_env_into_the_wrapped_command() {
        // Regression for issues #25/#49. NOTE: asserting on `wrapped.as_std().get_envs()` here would be
        // vacuous — `get_envs()` only reports explicit overrides, never whether `env_clear()` was called,
        // so it reads identically whether `wrapped.env_clear()` is present or not (confirmed against the
        // macOS sibling test: deleting that line still passed a `get_envs()`-based assertion there). This
        // spawns the REAL wrapped command through `bwrap` and inspects its actual environment, which DOES
        // differ — that's the only way to catch the composition bug this locks.
        if BwrapSandbox::detect().is_none() {
            return; // bwrap unavailable/non-functional on this host
        }
        let adapter = BwrapSandbox;
        let mut inner = tokio::process::Command::new("/usr/bin/env");
        inner.env_clear();
        inner.env("PATH", "/usr/bin");
        let mut wrapped = adapter
            .confine(inner, &policy(NetworkPolicy::Deny))
            .unwrap();
        let output = wrapped.output().await.expect("bwrap runs /usr/bin/env");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let key = line.split('=').next().unwrap_or(line);
            assert_eq!(
                key, "PATH",
                "the wrapped command must not inherit the full parent env, only the caller's explicit \
                 overrides: saw {stdout:?}"
            );
        }
        assert!(
            stdout.contains("PATH="),
            "the explicit override must still reach the child: {stdout:?}"
        );
    }
}
