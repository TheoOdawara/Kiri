use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::modules::tools::application::command_sandbox::{
    CommandSandbox, NetworkPolicy, SandboxPolicy,
};
use crate::shared::kernel::error::AgentError;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Credential directories under the user's home. Reads are denied even though the base profile allows
/// reads everywhere, so a confined command cannot read keys to exfiltrate them.
const SECRET_HOME_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"];

/// macOS OS-confinement adapter. Wraps the child in `sandbox-exec -p <profile>` with a generated
/// Seatbelt (SBPL) profile. A system binary — no FFI, no crate — so the crate-wide
/// `unsafe_code = "forbid"` lint is untouched. The profile shape is empirically verified on macOS:
/// `(deny network*)` blocks outbound connections, `(deny file-write* (subpath "/"))` plus targeted
/// allows confine writes to the workspace (and `/dev`, the temp dir, configured extras), and
/// `(deny file-read* …)` blocks credential stores.
///
/// `sandbox-exec` is Apple-deprecated but still shipped on current macOS; the long-term successor is
/// Endpoint Security (recorded in ADR 0009).
#[derive(Debug)]
pub struct MacosSeatbelt;

impl MacosSeatbelt {
    /// Available only when the system `sandbox-exec` binary is present.
    pub fn detect() -> Option<Self> {
        Path::new(SANDBOX_EXEC).exists().then_some(Self)
    }
}

impl CommandSandbox for MacosSeatbelt {
    fn confine(
        &self,
        cmd: tokio::process::Command,
        policy: &SandboxPolicy,
    ) -> Result<tokio::process::Command, AgentError> {
        // Fail closed: if the binary vanished since detection, refuse rather than spawn unconfined.
        if !Path::new(SANDBOX_EXEC).exists() {
            return Err(AgentError::Sandbox(format!(
                "{SANDBOX_EXEC} is unavailable; cannot confine the command"
            )));
        }
        // Read the built command's program/args/cwd/env back out (stdio is set later, at the single
        // spawn site), then rebuild it behind the sandbox-exec wrapper preserving all of them.
        let std = cmd.as_std();
        let program = std.get_program().to_owned();
        let args: Vec<OsString> = std.get_args().map(OsStr::to_owned).collect();
        let cwd = std.get_current_dir().map(Path::to_owned);
        let envs: Vec<(OsString, Option<OsString>)> = std
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
            .collect();

        let mut wrapped = tokio::process::Command::new(SANDBOX_EXEC);
        wrapped
            .arg("-p")
            .arg(build_profile(policy))
            .arg(program)
            .args(&args);
        if let Some(dir) = cwd {
            wrapped.current_dir(dir);
        }
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

/// Build the SBPL profile from the policy. The base is permissive (`allow default`) — the path-policy
/// and confirmation layers are the primary guard, so this OS layer only adds the guarantees they
/// cannot enforce: no outbound network for an untrusted command, no writes outside the workspace, and
/// no reads of credential stores. Every interpolated path is canonicalized (Seatbelt matches the real
/// path, and macOS routes `/var`→`/private/var`, `/tmp`→`/private/tmp`) and escaped.
fn build_profile(policy: &SandboxPolicy) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    if policy.network == NetworkPolicy::Deny {
        profile.push_str("(deny network*)\n");
    }

    // Writes: deny everything, then re-allow the platform essentials, the workspace root, and the
    // configured / per-call extras. Without `/dev` and the temp dir even `>/dev/null` and `mktemp`
    // (which build tools rely on) would fail.
    profile.push_str("(deny file-write* (subpath \"/\"))\n");
    push_allow_write(&mut profile, Path::new("/dev"));
    push_allow_write(&mut profile, Path::new("/private/tmp"));
    push_allow_write(&mut profile, &std::env::temp_dir());
    push_allow_write(&mut profile, &policy.root);
    for dir in &policy.extra_rw {
        push_allow_write(&mut profile, dir);
    }

    // Reads: deny credential directories under home (the base `allow default` would otherwise permit
    // them), so a confined command cannot read keys to exfiltrate. When HOME is unset there is no
    // per-user home whose credential dirs we could resolve, so there is nothing home-relative to deny —
    // the `~/.ssh`-style rules simply have no target. This skip is deliberate, not silent: HOME is
    // effectively always set on the macOS v1 target, so the empty case is the headless/CI edge and the
    // base posture stands.
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for dir in SECRET_HOME_DIRS {
            push_deny_read(&mut profile, &home.join(dir));
        }
    }
    // Re-allow any explicitly configured read paths (KIRI_SANDBOX_RO_PATHS), so the user can punch a
    // read-hole through the credential-dir denies above — e.g. a deploy tool that legitimately reads
    // ~/.aws/config. Emitted last so it wins (Seatbelt applies the last matching rule).
    for dir in &policy.extra_ro {
        push_allow_read(&mut profile, dir);
    }
    profile
}

fn push_allow_read(profile: &mut String, dir: &Path) {
    profile.push_str(&format!(
        "(allow file-read* (subpath \"{}\"))\n",
        sbpl_escape(&canon(dir))
    ));
}

fn push_allow_write(profile: &mut String, dir: &Path) {
    profile.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        sbpl_escape(&canon(dir))
    ));
}

fn push_deny_read(profile: &mut String, dir: &Path) {
    profile.push_str(&format!(
        "(deny file-read* (subpath \"{}\"))\n",
        sbpl_escape(&canon(dir))
    ));
}

/// Canonicalize for Seatbelt subpath matching, falling back to the input when the path does not yet
/// exist (e.g. a not-yet-created extra dir) so a rule is still emitted.
fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Escape a path for an SBPL string literal (`\` and `"`).
fn sbpl_escape(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
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

    #[test]
    fn profile_denies_network_when_requested() {
        let p = build_profile(&policy(NetworkPolicy::Deny));
        assert!(p.contains("(deny network*)"));
    }

    #[test]
    fn profile_allows_network_when_permitted() {
        let p = build_profile(&policy(NetworkPolicy::Allow));
        assert!(!p.contains("(deny network*)"));
    }

    #[test]
    fn profile_confines_writes_and_denies_secret_reads() {
        let p = build_profile(&policy(NetworkPolicy::Deny));
        assert!(p.contains("(deny file-write* (subpath \"/\"))"));
        assert!(p.contains("(allow file-write* (subpath \"/dev\"))"));
        // The configured extra is re-allowed for writing.
        assert!(p.contains("kiri-extra"));
        // Credential stores under home are read-denied.
        assert!(p.contains("file-read*") && p.contains(".ssh"));
    }

    #[tokio::test]
    async fn confine_wraps_the_command_in_sandbox_exec() {
        let adapter = MacosSeatbelt;
        let mut inner = tokio::process::Command::new("/bin/echo");
        inner.arg("hi");
        let wrapped = adapter
            .confine(inner, &policy(NetworkPolicy::Deny))
            .unwrap();
        let std = wrapped.as_std();
        assert_eq!(std.get_program(), SANDBOX_EXEC);
        let args: Vec<_> = std
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-p");
        assert!(args[1].contains("(deny network*)"));
        assert_eq!(args[2], "/bin/echo");
        assert_eq!(args[3], "hi");
    }
}
