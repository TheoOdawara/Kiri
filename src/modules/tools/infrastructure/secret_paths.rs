//! Single source of truth for the credential paths the tool layer must protect. Three enforcement
//! layers consume these — the path-resolution guard (`sandbox::secret_dir_component`, which refuses to
//! operate inside one of these directories), the macOS Seatbelt read-deny (`confine::macos`), and the
//! Linux bwrap shadow (`confine::linux`). They were byte-identical duplicated lists with nothing tying
//! them together; single-sourcing here means a future addition lands in every layer at once instead of
//! silently weakening one (SEC-03/TOOL-04).

/// Directory names that hold credentials/keys. Every path resolution refuses to operate *inside* one
/// of these, since the file-name sensitive guard matches files, not directories — without this,
/// `search` could recurse into `~/.ssh` and print `id_rsa` line by line, and `read_file` could read
/// `~/.aws/config` (a non-sensitive *name* in a secret *dir*). Re-exported from `sandbox` so
/// `search`/`run_command` keep resolving `sandbox::SECRET_DIRS`. Compared case-insensitively (macOS APFS).
pub(crate) const SECRET_DIRS: &[&str] = &[
    ".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker", ".azure", ".gcloud",
];

/// Well-known credential files directly under the user's home, denied to confined children by the macOS
/// Seatbelt profile and shadowed by the Linux bwrap adapter. They mirror names already in
/// `DEFAULT_SENSITIVE_PATTERNS`, but that file-name guard only covers the file tools — `run_command`'s
/// free-form shell reaches these through the OS layer alone.
// Only consumed by the macOS/Linux OS-confinement adapters, so it is dead on other targets (Windows'
// `clippy --all-targets -D warnings` gate would otherwise reject it).
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) const HOME_SECRET_FILES: &[&str] =
    &[".netrc", ".npmrc", ".pypirc", ".pgpass", ".git-credentials"];

/// Multi-component secret stores under home that must not be blanket-denied via a single top-level
/// name (denying all of `.config` would be far too broad). Each entry is a sequence of path
/// components under `$HOME` (e.g. `[".config", "gh"]` → `~/.config/gh`). Consumed by path resolution
/// and the macOS/Linux OS-confinement adapters.
pub(crate) const HOME_SECRET_SUBPATHS: &[&[&str]] = &[&[".config", "gh"], &[".config", "gcloud"]];

/// The harness's own private directory under home (`~/.kiri`), which holds `credentials.json` (the
/// `0600` API-key store) and other state. Denied to confined children so a `run_command` cannot read
/// it back to the model, and denied in file-tool path resolution (not via `SECRET_DIRS` — project
/// workspaces legitimately use a local `.kiri/`).
pub(crate) const HARNESS_PRIVATE_DIR: &str = ".kiri";
