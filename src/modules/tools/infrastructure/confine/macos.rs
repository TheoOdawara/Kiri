use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};
use crate::modules::tools::infrastructure::secret_paths::{
    HARNESS_PRIVATE_DIR, HOME_SECRET_FILES, SECRET_DIRS,
};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::sandbox::NetworkPolicy;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// macOS OS-confinement adapter. Wraps the child in `sandbox-exec -p <profile>` with a generated
/// Seatbelt (SBPL) profile. A system binary — no FFI, no crate — so the crate-wide
/// `unsafe_code = "forbid"` lint is untouched. The profile shape is empirically verified on macOS:
/// `(deny network*)` blocks outbound connections, `(deny file-write* (subpath "/"))` plus targeted
/// allows confine writes to the workspace (and `/dev`, the temp dir, configured extras), and
/// `(deny file-read* …)` blocks credential stores — the single-sourced `SECRET_DIRS` (`~/.ssh`,
/// `~/.aws`, …), the harness's own `~/.kiri` (which holds `credentials.json`), and the well-known home
/// credential files (`~/.netrc`, `~/.git-credentials`, …) — so a confined `run_command` cannot read
/// them back to the model.
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

    // Credential set under home (the base `allow default` would otherwise permit it). When HOME is unset
    // there is no per-user home whose credential paths we could resolve, so there is nothing home-relative
    // to deny. This skip is deliberate, not silent: HOME is effectively always set on the macOS v1 target,
    // so the empty case is the headless/CI edge and the base posture stands.
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        // Writes: deny the credential set, emitted AFTER the write-allows above so they win even when the
        // workspace root (or a configured `extra_rw`) is a home ancestor (e.g. after `/cd ~`), where
        // `(allow file-write* (subpath <root>))` would otherwise cover all of home (last-match-wins) and
        // let a confined command clobber `~/.ssh/authorized_keys` or `~/.kiri/credentials.json`. Unlike
        // reads (the `extra_ro` allow below can punch a read hole), credential writes are never
        // punch-through-able — clobbering a credential store is strictly more dangerous (SEC-03).
        push_home_write_denies(&mut profile, &home);
        // Reads: deny the same set so a confined command cannot read keys to exfiltrate.
        push_home_denies(&mut profile, &home);
    }
    // Re-allow any explicitly configured read paths (KIRI_SANDBOX_RO_PATHS), so the user can punch a
    // read-hole through the credential-dir denies above — e.g. a deploy tool that legitimately reads
    // ~/.aws/config. Emitted last so it wins (Seatbelt applies the last matching rule).
    for dir in &policy.extra_ro {
        push_allow_read(&mut profile, dir);
    }
    profile
}

/// Emit the credential read-denies resolved relative to `home`: the single-sourced credential dirs, the
/// harness's own `~/.kiri` store, and the well-known home credential files. Pure in `home`, so it is
/// testable with a fixed home without mutating the process environment (edition-2024-safe).
fn push_home_denies(profile: &mut String, home: &Path) {
    for dir in SECRET_DIRS {
        push_deny_read(profile, &home.join(dir));
    }
    push_deny_read(profile, &home.join(HARNESS_PRIVATE_DIR));
    for file in HOME_SECRET_FILES {
        push_deny_read(profile, &home.join(file));
    }
}

/// The write-side mirror of `push_home_denies`: refuse writes into the same credential set, so a confined
/// command cannot clobber `~/.ssh/authorized_keys`, `~/.aws/credentials`, or `~/.kiri/credentials.json`
/// when the workspace root is a home ancestor. Pure in `home`, like its read sibling (SEC-03).
fn push_home_write_denies(profile: &mut String, home: &Path) {
    for dir in SECRET_DIRS {
        push_deny_write(profile, &home.join(dir));
    }
    push_deny_write(profile, &home.join(HARNESS_PRIVATE_DIR));
    for file in HOME_SECRET_FILES {
        push_deny_write(profile, &home.join(file));
    }
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

fn push_deny_write(profile: &mut String, dir: &Path) {
    profile.push_str(&format!(
        "(deny file-write* (subpath \"{}\"))\n",
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
        // Credential stores under home are read-denied: the dirs (`.ssh`), the harness's own `~/.kiri`
        // (which holds `credentials.json`), and the well-known home credential files.
        assert!(p.contains("file-read*"));
        assert!(p.contains(".ssh"));
        assert!(p.contains(".kiri"));
        assert!(p.contains(".netrc"));
        assert!(p.contains(".git-credentials"));
    }

    #[test]
    fn profile_denies_kiri_credential_store() {
        // Drive the pure helper with a fixed home so the assertion is deterministic (no reliance on the
        // ambient HOME): `~/.kiri` — the harness's `credentials.json` store — must be read-denied.
        let mut profile = String::new();
        push_home_denies(&mut profile, Path::new("/Users/fake"));
        assert!(profile.contains("(deny file-read* (subpath \"/Users/fake/.kiri\"))"));
        assert!(profile.contains("/Users/fake/.netrc"));
        assert!(profile.contains("/Users/fake/.ssh"));
    }

    #[test]
    fn home_credential_denies_survive_an_empty_read_extra_but_a_home_ancestor_overrides_them() {
        // Regression (TOOL-07): read-only tools used to route their per-call cwd through `extra_ro`,
        // which `build_profile` emits LAST (last-match-wins). When the workspace root was a home
        // ancestor (e.g. after `/cd ~`), the emitted `(allow file-read* (subpath $HOME))` overrode the
        // credential denies, so a confined `search`/`read_file` could read `~/.kiri/credentials.json`.
        // The fix makes read-only tools pass an empty `extra_ro`, so no overriding allow is emitted.
        let Some(home) = std::env::var_os("HOME") else {
            return; // headless/CI edge: no per-user home whose credential denies exist
        };
        let home = PathBuf::from(home);

        // The exact deny/allow fragments `build_profile` emits, built via the same helpers so the match
        // is byte-identical rather than a hand-rolled approximation.
        let mut kiri_deny = String::new();
        push_deny_read(&mut kiri_deny, &home.join(HARNESS_PRIVATE_DIR));
        let kiri_deny = kiri_deny.trim_end();
        let mut home_allow = String::new();
        push_allow_read(&mut home_allow, &home);
        let home_allow = home_allow.trim_end();

        // Fixed read-only shape (empty extra_ro): the `~/.kiri` deny stands, with no overriding allow.
        let fixed = build_profile(&SandboxPolicy {
            root: home.clone(),
            network: NetworkPolicy::Deny,
            extra_ro: Vec::new(),
            extra_rw: Vec::new(),
        });
        assert!(
            fixed.contains(kiri_deny),
            "the ~/.kiri credential deny must be present"
        );
        assert!(
            !fixed.contains(home_allow),
            "no read-allow may override the credential deny when extra_ro is empty"
        );

        // Old buggy shape (per-call cwd == home ancestor fed through extra_ro): the override returns —
        // the home read-allow is emitted AFTER the deny, so the credential file becomes readable. This
        // is exactly the path the call-site fix removed.
        let buggy = build_profile(&SandboxPolicy {
            root: home.clone(),
            network: NetworkPolicy::Deny,
            extra_ro: vec![home.clone()],
            extra_rw: Vec::new(),
        });
        let deny_at = buggy
            .find(kiri_deny)
            .expect("deny present in the buggy shape");
        let allow_at = buggy
            .find(home_allow)
            .expect("override allow present in the buggy shape");
        assert!(
            allow_at > deny_at,
            "the buggy shape emits the overriding allow after the deny (last-match-wins)"
        );
    }

    #[test]
    fn credential_write_denies_win_when_the_root_is_a_home_ancestor() {
        // SEC-03: with the workspace root == home, the root write-allow `(allow file-write* (subpath
        // $HOME))` would cover the whole credential set. The credential write-deny must be emitted AFTER
        // that allow (last-match-wins), so `~/.kiri`/`~/.ssh` stay write-protected even after `/cd ~`.
        let Some(home) = std::env::var_os("HOME") else {
            return; // headless/CI edge: no per-user home whose credential denies exist
        };
        let home = PathBuf::from(home);

        let mut kiri_write_deny = String::new();
        push_deny_write(&mut kiri_write_deny, &home.join(HARNESS_PRIVATE_DIR));
        let kiri_write_deny = kiri_write_deny.trim_end();
        let mut home_write_allow = String::new();
        push_allow_write(&mut home_write_allow, &home);
        let home_write_allow = home_write_allow.trim_end();

        let profile = build_profile(&SandboxPolicy {
            root: home.clone(),
            network: NetworkPolicy::Deny,
            extra_ro: Vec::new(),
            extra_rw: Vec::new(),
        });

        let allow_at = profile
            .find(home_write_allow)
            .expect("the root write-allow is present");
        let deny_at = profile
            .find(kiri_write_deny)
            .expect("the credential write-deny must be present");
        assert!(
            deny_at > allow_at,
            "the credential write-deny must come AFTER the root write-allow so it wins (last-match-wins)"
        );
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
