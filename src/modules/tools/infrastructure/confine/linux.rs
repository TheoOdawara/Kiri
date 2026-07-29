use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::modules::tools::application::command_sandbox::{
    CommandSandbox, SandboxGuarantees, SandboxPolicy, WorkspaceAccess,
};
use crate::modules::tools::infrastructure::secret_paths::{
    HARNESS_PRIVATE_DIR, HOME_SECRET_FILES, HOME_SECRET_SUBPATH_FILES, HOME_SECRET_SUBPATHS,
    SECRET_DIRS,
};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::sandbox::NetworkPolicy;

const BWRAP: &str = "/usr/bin/bwrap";
const SYSTEM_READ_ROOTS: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"];

/// Linux OS-confinement adapter. Wraps the child in `bwrap <flags> -- <program> <args…>`, mirroring the
/// macOS Seatbelt adapter's argv-transform shape. A system binary — no FFI, no crate — so the crate-wide
/// `unsafe_code = "forbid"` lint is untouched. The guest root is not mounted: required system roots and
/// configured paths are added explicitly, while `--tmpfs`/`--ro-bind /dev/null` shadow any credential
/// path that an explicit mount would otherwise expose.
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

/// Run bwrap against `/bin/true` inside the *actual* jail shape `confine()` builds — via the same
/// `build_args` — and require exit 0. It must be representative, not a lenient subset: an environment
/// where a minimal mount jail runs but the full jail (`--proc`, `--tmpfs`, a writable `--bind`,
/// the credential shadows) does not — e.g. a hardened CI runner that permits only a partial unprivileged
/// user namespace — would otherwise pass detection yet fail every real confined command. A cheap,
/// synchronous check — called once at startup (`detect`) and again on every `confine()` call (fail-closed).
fn probe() -> bool {
    let policy = SandboxPolicy {
        root: std::env::temp_dir(),
        command_home: std::env::temp_dir(),
        workspace_access: WorkspaceAccess::ReadWrite,
        network: NetworkPolicy::Deny,
        extra_ro: Vec::new(),
        extra_rw: Vec::new(),
    };
    let mut args = build_args(&policy, None);
    args.push(OsString::from("--"));
    args.push(OsString::from("/bin/true"));
    std::process::Command::new(BWRAP)
        .args(&args)
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

    fn guarantees(&self) -> SandboxGuarantees {
        SandboxGuarantees::BWRAP
    }
}

/// Build the bwrap flag list from the policy. bwrap applies binds in argument order: a later bind
/// shadows an earlier one at the same or a nested path. System roots are mounted read-only, followed by
/// the workspace/extras, credential shadows, and explicit read re-allows last.
fn build_args(policy: &SandboxPolicy, cwd: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();

    for root in SYSTEM_READ_ROOTS
        .iter()
        .map(Path::new)
        .filter(|path| path.exists())
    {
        push_self_bind(&mut args, "--ro-bind", root);
    }
    push_dest_only(&mut args, "--dev", Path::new("/dev"));
    push_dest_only(&mut args, "--proc", Path::new("/proc"));
    push_dest_only(&mut args, "--tmpfs", &std::env::temp_dir());

    let workspace_flag = match policy.workspace_access {
        WorkspaceAccess::ReadOnly => "--ro-bind",
        WorkspaceAccess::ReadWrite => "--bind",
    };
    push_self_bind(&mut args, workspace_flag, &policy.root);
    for dir in &policy.extra_rw {
        if dir.exists() {
            push_self_bind(&mut args, "--bind", dir);
        }
    }

    // Credential set under home, shadowed AFTER the write-allows above so they win even when the
    // workspace root (or an `extra_rw`) is a home ancestor (e.g. after `/cd ~`) - mirrors the macOS
    // adapter's last-match-wins credential denies. A directory is shadowed with an empty `--tmpfs`
    // (looks empty but still traversable, so the parent directory listing does not break); a single
    // file is shadowed with `--ro-bind /dev/null` (reads as empty, cannot be written).
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for dir in SECRET_DIRS {
            let target = home.join(dir);
            if is_exposed(&target, policy) {
                push_dest_only(&mut args, "--tmpfs", &target);
            }
        }
        let harness = home.join(HARNESS_PRIVATE_DIR);
        if is_exposed(&harness, policy) {
            push_dest_only(&mut args, "--tmpfs", &harness);
        }
        for components in HOME_SECRET_SUBPATHS {
            let mut sub = home.clone();
            for component in *components {
                sub.push(component);
            }
            if is_exposed(&sub, policy) {
                push_dest_only(&mut args, "--tmpfs", &sub);
            }
        }
        for file in HOME_SECRET_FILES {
            let target = home.join(file);
            if target.exists() && is_exposed(&target, policy) {
                push_shadow_file(&mut args, &target);
            }
        }
        for components in HOME_SECRET_SUBPATH_FILES {
            let target = components
                .iter()
                .fold(home.clone(), |path, component| path.join(component));
            if target.exists() && is_exposed(&target, policy) {
                push_shadow_file(&mut args, &target);
            }
        }
    }

    // Re-allow any explicitly configured read paths (KIRI_SANDBOX_RO_PATHS), so the user can punch a
    // read-hole through the credential shadows above. Emitted last so it wins (bwrap applies binds in
    // argument order).
    for dir in &policy.extra_ro {
        if dir.exists() {
            push_self_bind(&mut args, "--ro-bind", dir);
        }
    }
    // The exact synthetic home is the final filesystem grant. This restores only that managed subtree
    // when a broader workspace/extra mount caused the `~/.kiri` credential shadow above to cover it.
    push_self_bind(&mut args, "--bind", &policy.command_home);

    if policy.network == NetworkPolicy::Deny {
        args.push(OsString::from("--unshare-net"));
    }
    args.extend([
        OsString::from("--unshare-pid"),
        OsString::from("--unshare-ipc"),
        OsString::from("--new-session"),
        OsString::from("--cap-drop"),
        OsString::from("ALL"),
    ]);
    args.push(OsString::from("--die-with-parent"));
    if let Some(dir) = cwd {
        push_dest_only(&mut args, "--chdir", dir);
    }
    args
}

/// `--bind`/`--ro-bind SRC DEST` where the destination is the same path as the source: the shape every
/// bind in this adapter uses, preserving the source's absolute location inside the jail.
fn push_self_bind(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    let resolved = canon(path);
    args.push(OsString::from(flag));
    args.push(resolved.into_os_string());
    args.push(path.to_owned().into_os_string());
}

fn is_exposed(path: &Path, policy: &SandboxPolicy) -> bool {
    path.starts_with(&policy.root)
        || path.starts_with(&policy.command_home)
        || policy.extra_ro.iter().any(|root| path.starts_with(root))
        || policy.extra_rw.iter().any(|root| path.starts_with(root))
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
    use std::ops::{Deref, DerefMut};

    struct PolicyFixture {
        _directory: tempfile::TempDir,
        policy: SandboxPolicy,
    }

    impl Deref for PolicyFixture {
        type Target = SandboxPolicy;

        fn deref(&self) -> &Self::Target {
            &self.policy
        }
    }

    impl DerefMut for PolicyFixture {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.policy
        }
    }

    fn policy(network: NetworkPolicy, workspace_access: WorkspaceAccess) -> PolicyFixture {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("kiri-ws");
        let command_home = directory.path().join("kiri-home");
        let extra = directory.path().join("kiri-extra");
        for path in [&root, &command_home, &extra] {
            std::fs::create_dir(path).unwrap();
        }
        PolicyFixture {
            _directory: directory,
            policy: SandboxPolicy {
                root,
                command_home,
                workspace_access,
                network,
                extra_ro: Vec::new(),
                extra_rw: vec![extra],
            },
        }
    }

    fn args_to_strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn args_unshare_net_when_denied() {
        let args = args_to_strings(&build_args(
            &policy(NetworkPolicy::Deny, WorkspaceAccess::ReadWrite),
            None,
        ));
        assert!(args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn args_allow_network_when_permitted() {
        let args = args_to_strings(&build_args(
            &policy(NetworkPolicy::Allow, WorkspaceAccess::ReadWrite),
            None,
        ));
        assert!(!args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn read_write_policy_binds_the_workspace_and_extras_writable() {
        let args = args_to_strings(&build_args(
            &policy(NetworkPolicy::Deny, WorkspaceAccess::ReadWrite),
            None,
        ));
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
    fn read_only_policy_never_binds_the_workspace_writable() {
        let args = args_to_strings(&build_args(
            &policy(NetworkPolicy::Deny, WorkspaceAccess::ReadOnly),
            None,
        ));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--ro-bind" && w[1].contains("kiri-ws"))
        );
        assert!(
            !args
                .windows(2)
                .any(|w| w[0] == "--bind" && w[1].contains("kiri-ws"))
        );
    }

    #[test]
    fn minimal_jail_does_not_mount_the_host_root() {
        let args = args_to_strings(&build_args(
            &policy(NetworkPolicy::Deny, WorkspaceAccess::ReadOnly),
            None,
        ));
        assert!(
            !args
                .windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == "/" && w[2] == "/")
        );
    }

    #[test]
    fn jail_enables_process_ipc_session_and_capability_isolation() {
        let args = args_to_strings(&build_args(
            &policy(NetworkPolicy::Deny, WorkspaceAccess::ReadOnly),
            None,
        ));
        for flag in [
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--new-session",
            "--die-with-parent",
        ] {
            assert!(args.contains(&flag.to_string()), "missing {flag}");
        }
        assert!(
            args.windows(2)
                .any(|window| window == ["--cap-drop", "ALL"])
        );
    }

    #[test]
    fn args_shadow_credential_dirs_after_write_allows() {
        let Some(home) = std::env::var_os("HOME") else {
            return; // headless/CI edge: no per-user home whose credential shadows exist
        };
        let home = PathBuf::from(home);
        let mut policy = policy(NetworkPolicy::Deny, WorkspaceAccess::ReadWrite);
        policy.root = home.clone();
        policy.command_home = home.join(".kiri/sandbox/workspaces/test/home");
        let args = build_args(&policy, None);
        let strings = args_to_strings(&args);
        let root_bind_at = strings
            .iter()
            .position(|a| a == home.to_string_lossy().as_ref())
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
        let command_home_at = strings
            .iter()
            .rposition(|argument| argument == policy.command_home.to_string_lossy().as_ref())
            .expect("synthetic home bind present");
        let harness_shadow_at = strings
            .iter()
            .position(|argument| argument == home.join(".kiri").to_string_lossy().as_ref())
            .expect("harness shadow present");
        assert!(command_home_at > harness_shadow_at);
    }

    #[test]
    fn args_reallow_configured_read_paths_last() {
        let mut p = policy(NetworkPolicy::Deny, WorkspaceAccess::ReadWrite);
        p.extra_ro.push(std::env::temp_dir());
        let args = build_args(&p, None);
        let strings = args_to_strings(&args);
        let last_ro_bind = strings
            .iter()
            .enumerate()
            .rfind(|(_, a)| a.as_str() == "--ro-bind")
            .map(|(i, _)| i)
            .expect("at least one --ro-bind present");
        // The last --ro-bind flag emitted must be the explicit extra_ro re-allow.
        assert_ne!(strings[last_ro_bind + 1], "/");
    }

    #[test]
    fn args_include_chdir_when_cwd_given() {
        let dir = std::env::temp_dir();
        let args = args_to_strings(&build_args(
            &policy(NetworkPolicy::Deny, WorkspaceAccess::ReadWrite),
            Some(&dir),
        ));
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
            .confine(
                inner,
                &policy(NetworkPolicy::Deny, WorkspaceAccess::ReadWrite),
            )
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
        let fixture = policy(NetworkPolicy::Deny, WorkspaceAccess::ReadWrite);
        let mut wrapped = adapter.confine(inner, &fixture).unwrap();
        let output = wrapped.output().await.expect("bwrap runs /usr/bin/env");
        assert!(output.status.success(), "bwrap failed: {output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let actual: std::collections::BTreeSet<_> = stdout.lines().collect();
        let expected = std::collections::BTreeSet::from(["PATH=/usr/bin", "PWD=/"]);
        assert_eq!(
            actual, expected,
            "the wrapped command must receive only the caller's PATH and bwrap's synthetic PWD"
        );
    }

    #[tokio::test]
    async fn read_only_workspace_rejects_a_real_write() {
        if BwrapSandbox::detect().is_none() {
            return; // CI's mandatory bwrap smoke makes this a hard failure there
        }
        let adapter = BwrapSandbox;
        let fixture = policy(NetworkPolicy::Deny, WorkspaceAccess::ReadOnly);
        let target = fixture.root.join("must-not-exist");
        let mut inner = tokio::process::Command::new("/usr/bin/touch");
        inner.arg(&target).env_clear();
        let mut wrapped = adapter.confine(inner, &fixture).unwrap();

        let output = wrapped.output().await.expect("bwrap runs touch");

        assert!(!output.status.success());
        assert!(!target.exists());
    }
}
