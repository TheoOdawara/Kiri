use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};
use crate::modules::tools::application::path::{
    anchor_to_root_drive, expand_tilde, home, is_absolute_path,
};
use crate::modules::tools::application::sandbox::{CreateResolution, Sandbox};
#[cfg(test)]
use crate::modules::tools::infrastructure::confine::noop::NoConfinement;
use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::sandbox::NetworkPolicy;

/// The credential-directory names guarded by both the path-resolution refusal here and the macOS
/// Seatbelt read-deny. Single-sourced in `secret_paths` and re-exported so existing importers
/// (`search.rs`, `run_command.rs`) keep resolving `sandbox::SECRET_DIRS` unchanged.
pub(crate) use crate::modules::tools::infrastructure::secret_paths::SECRET_DIRS;
use crate::modules::tools::infrastructure::secret_paths::{
    HARNESS_PRIVATE_DIR, HOME_SECRET_SUBPATHS,
};

/// Confines every file operation to a canonicalized root directory, and refuses CRUD on files
/// whose name matches a sensitive pattern (secrets, keys, credentials). All file tools resolve
/// their path through this type; nothing else touches the filesystem with a raw, unvalidated path.
///
// ponytail: the path-resolution methods (`with_confinement`, `resolve_existing`, `resolve_create`) run
// blocking `std::fs` calls (canonicalize/exists/is_dir) directly on the single-threaded TUI runtime,
// with no timeout — accepted because the workspace root is a LOCAL filesystem, where these calls
// return promptly. Upgrade path if a remote/automount fs ever backs the root: make the `Sandbox` port
// async and route each call through `run_blocking_with_timeout` (shared/infra/sqlite), as the SQLite
// adapter does. Not done now: a sync→async port flip ripples to every tool, disproportionate to a
// local-fs hang.
#[derive(Debug, Clone)]
pub struct FsSandbox {
    root: PathBuf,
    sensitive: SensitiveMatcher,
    /// OS-level confinement applied to every child process the tools spawn. `NoConfinement` on
    /// platforms without a facility (and `KIRI_SANDBOX=off`), the platform adapter otherwise.
    confiner: Arc<dyn CommandSandbox>,
    /// The sandbox's network stance — no per-command widening (ADR 0022).
    network: NetworkPolicy,
    /// Extra paths a confined command may read / write beyond the workspace (toolchain dirs, config).
    extra_ro: Arc<[PathBuf]>,
    extra_rw: Arc<[PathBuf]>,
}

impl FsSandbox {
    /// Canonicalize the root once. Fails if it does not exist or is not a directory. The
    /// `SensitiveMatcher` is carried on the sandbox so every path resolution can check the file
    /// name against the sensitive patterns before the tool touches the filesystem.
    /// Unconfined shorthand (no OS sandbox, deny-network policy, no extras). Used by tests; the
    /// composition root builds the real sandbox via `with_confinement`.
    #[cfg(test)]
    pub fn new(root: impl AsRef<Path>, sensitive: SensitiveMatcher) -> Result<Self> {
        Self::with_confinement(
            root,
            sensitive,
            Arc::new(NoConfinement),
            NetworkPolicy::Deny,
            Arc::from(Vec::new()),
            Arc::from(Vec::new()),
        )
    }

    /// Build a sandbox with an explicit OS-confinement adapter and policy extras. The composition root
    /// uses this; `new` is the unconfined shorthand (tests, and the default before `app::wire`).
    pub fn with_confinement(
        root: impl AsRef<Path>,
        sensitive: SensitiveMatcher,
        confiner: Arc<dyn CommandSandbox>,
        network: NetworkPolicy,
        extra_ro: Arc<[PathBuf]>,
        extra_rw: Arc<[PathBuf]>,
    ) -> Result<Self> {
        let root = root.as_ref();
        let canonical = std::fs::canonicalize(root)
            .with_context(|| format!("sandbox root {} does not exist", root.display()))?;
        if !canonical.is_dir() {
            bail!("sandbox root {} is not a directory", canonical.display());
        }
        Ok(Self {
            root: canonical,
            sensitive,
            confiner,
            network,
            extra_ro,
            extra_rw,
        })
    }

    /// Resolve a new workspace root from a `/cd` argument and return the relocated sandbox. A relative
    /// argument is joined onto the current root; an absolute or `~`/`~/…` argument is taken as given.
    /// The new root is canonicalized and must exist and be a directory (else this fails). Returns
    /// `Self`, so it stays inherent on the concrete adapter rather than on the object-safe port.
    pub fn relocated(&self, arg: &str) -> Result<Self> {
        let expanded = expand_tilde(arg, home().as_deref());
        let target = if is_absolute_path(arg, &expanded) {
            anchor_to_root_drive(&expanded, &self.root)
        } else {
            self.root.join(arg)
        };
        Self::with_confinement(
            &target,
            self.sensitive.clone(),
            self.confiner.clone(),
            self.network,
            self.extra_ro.clone(),
            self.extra_rw.clone(),
        )
    }
}

impl Sandbox for FsSandbox {
    fn root(&self) -> &Path {
        &self.root
    }

    /// The OS-confinement adapter every tool wraps its child process with before spawning.
    fn confiner(&self) -> &dyn CommandSandbox {
        self.confiner.as_ref()
    }

    /// The sandbox's network stance — the sole source of truth for `run_command`, with no per-command
    /// widening (ADR 0022).
    fn network(&self) -> NetworkPolicy {
        self.network
    }

    /// Build the per-call OS-confinement policy: fold the per-call `extra_ro`/`extra_rw` into the
    /// configured read/write extras. Writes stay confined to the workspace root plus the write extras;
    /// the read extras only re-allow reads (a read-only tool's cwd lands here, not in the write set).
    fn command_policy(
        &self,
        network: NetworkPolicy,
        extra_ro: &[&Path],
        extra_rw: &[&Path],
    ) -> SandboxPolicy {
        let mut ro: Vec<PathBuf> = self.extra_ro.to_vec();
        ro.extend(extra_ro.iter().map(|path| path.to_path_buf()));
        let mut rw: Vec<PathBuf> = self.extra_rw.to_vec();
        rw.extend(extra_rw.iter().map(|path| path.to_path_buf()));
        SandboxPolicy {
            root: self.root.clone(),
            network,
            extra_ro: ro,
            extra_rw: rw,
        }
    }

    /// The name of a secret directory (`.ssh`, `.aws`, …) that `real` lies within, if any. The
    /// recursive read-only tools use this to refuse poking inside a credential store, which the
    /// file-name guard alone does not cover.
    fn secret_dir_component(&self, real: &Path) -> Option<&'static str> {
        real.components().find_map(|component| {
            let Component::Normal(name) = component else {
                return None;
            };
            let name = name.to_str()?;
            SECRET_DIRS
                .iter()
                .copied()
                .find(|dir| dir.eq_ignore_ascii_case(name))
        })
    }

    /// Resolve a path that must already exist (read/edit/overwrite/delete/list/search). A relative path
    /// resolves under the active root and is asserted to stay within it; an absolute path (or `~/…`) is
    /// resolved as given — allowed outside the root, since the CLI gates it with explicit confirmation.
    fn resolve_existing(&self, rel: &str) -> Result<PathBuf, AgentError> {
        reject_ads_syntax(rel)?;
        let expanded = expand_tilde(rel, home().as_deref());
        if is_absolute_path(rel, &expanded) {
            let target = anchor_to_root_drive(&expanded, &self.root);
            let real = std::fs::canonicalize(&target)
                .map_err(|_| AgentError::Sandbox(format!("path not found: {rel}")))?;
            self.assert_not_sensitive(&real, rel)?;
            self.assert_not_in_secret_dir(&real, rel)?;
            self.assert_not_under_harness_private(&real, rel)?;
            self.assert_not_under_home_secret_subpaths(&real, rel)?;
            return Ok(real);
        }
        let candidate = self.join_checked(rel)?;
        let real = std::fs::canonicalize(&candidate)
            .map_err(|_| AgentError::Sandbox(format!("path not found: {rel}")))?;
        self.assert_within(&real)?;
        self.assert_not_sensitive(&real, rel)?;
        self.assert_not_in_secret_dir(&real, rel)?;
        self.assert_not_under_harness_private(&real, rel)?;
        self.assert_not_under_home_secret_subpaths(&real, rel)?;
        Ok(real)
    }

    /// Resolve a path for creation. The target need not exist and intermediate directories may be
    /// missing. The deepest existing ancestor is canonicalized and asserted within the root, so the
    /// remaining (lexically clean) components appended onto it cannot escape.
    ///
    /// Security model: a relative path is confined to the root (relative-path traversal cannot escape).
    /// An absolute path — including a tilde that expands to one — is taken as an explicit, out-of-root
    /// location the user must approve at the confirmation prompt, so it deliberately bypasses the
    /// within-root check (`confined == false`). It is still run through `assert_not_sensitive`, so
    /// secret paths are rejected regardless.
    fn resolve_create(&self, rel: &str) -> Result<CreateResolution, AgentError> {
        reject_ads_syntax(rel)?;
        let expanded = expand_tilde(rel, home().as_deref());
        let (candidate, confined) = if is_absolute_path(rel, &expanded) {
            // Absolute/tilde-expanded paths are user-approved out-of-root targets — not confined here;
            // see the security model above.
            (anchor_to_root_drive(&expanded, &self.root), false)
        } else {
            (self.join_checked(rel)?, true)
        };
        if confined && candidate == self.root {
            return Err(AgentError::Sandbox(format!("path has no file name: {rel}")));
        }

        let mut existing = candidate.as_path();
        let mut missing: Vec<&Path> = Vec::new();
        while !existing.exists() {
            missing.push(existing);
            existing = existing
                .parent()
                .ok_or_else(|| AgentError::Sandbox(format!("invalid path: {rel}")))?;
        }

        let real_existing = std::fs::canonicalize(existing).map_err(|_| {
            AgentError::Sandbox(format!("cannot resolve an existing ancestor of {rel}"))
        })?;
        if confined {
            self.assert_within(&real_existing)?;
        }

        // `missing` is deepest-first; reverse to shallow-first for both the join and mkdir order.
        missing.reverse();
        let mut real_target = real_existing;
        let mut missing_dirs = Vec::new();
        for (idx, segment) in missing.iter().enumerate() {
            let name = segment
                .file_name()
                .ok_or_else(|| AgentError::Sandbox(format!("invalid path component in {rel}")))?;
            real_target = real_target.join(name);
            // Every component but the last one is a directory that must be created.
            if idx + 1 < missing.len() {
                missing_dirs.push(real_target.clone());
            }
        }

        self.assert_not_sensitive(&real_target, rel)?;
        self.assert_not_in_secret_dir(&real_target, rel)?;
        self.assert_not_under_harness_private(&real_target, rel)?;
        self.assert_not_under_home_secret_subpaths(&real_target, rel)?;
        Ok(CreateResolution {
            target: real_target,
            missing_dirs,
        })
    }

    /// Whether a bare file name matches a sensitive pattern (secrets, keys, credentials). Exposing the
    /// boolean keeps the `SensitiveMatcher` itself out of the application-layer port.
    fn is_sensitive_name(&self, name: &str) -> bool {
        self.sensitive.matches(name).is_some()
    }
}

/// Reject any path component containing a literal `:`. NTFS Alternate Data Stream syntax
/// (`filename:streamname`) addresses a DIFFERENT byte stream of the SAME underlying file without the
/// component's text matching the base file name at all — `id_rsa::$DATA` reads the exact same bytes as
/// `id_rsa`, but as a path component its name is the whole string `"id_rsa::$DATA"`, which the sensitive-
/// name matcher's anchored glob (`^id_rsa$`) does not match, and which does not equal any credential-
/// directory name either. `std::path` never splits a component on `:` — it only recognizes a drive-letter
/// PREFIX (`Component::Prefix`, not `Component::Normal`) at an absolute path's very start — so this checks
/// the raw component text directly (issue #26).
///
/// Applied on every platform, not gated to `#[cfg(windows)]`: a literal colon in a bare file/dir name is
/// vanishingly rare on any supported OS (and NTFS itself forbids one in an ordinary name outside this
/// exact syntax), so refusing it outright is a safe, simple default-deny rather than trying to parse and
/// selectively strip a stream suffix — which would also need to handle the bare `filename:` form (default
/// `::$DATA` stream, no explicit suffix) and the `filename::$INDEX_ALLOCATION` directory-stream form.
///
/// `Component::Prefix` (the drive-letter form, e.g. `C:`) only exists in `std::path`'s Windows backend —
/// on this project's current macOS/Linux dev and CI hosts, a string like `C:\Users\x` parses as ordinary
/// `Component::Normal` segments (backslash is not a separator there either), so the drive-letter exemption
/// this function relies on cannot be exercised by this test suite today. Untested by construction, not by
/// oversight — add a `#[cfg(windows)]` regression once Windows is an actual build target (ADR-tracked
/// distribution strategy: macOS is v1, Windows/Linux later) so a future edit to the `Component::Normal`
/// guard can't silently start refusing legitimate Windows drive paths with zero test signal.
fn reject_ads_syntax(rel: &str) -> Result<(), AgentError> {
    for component in Path::new(rel).components() {
        if let Component::Normal(segment) = component
            && segment.to_str().is_some_and(|s| s.contains(':'))
        {
            return Err(AgentError::Sandbox(format!(
                "path component contains ':' (NTFS alternate-data-stream syntax is not allowed): {rel}"
            )));
        }
    }
    Ok(())
}

impl FsSandbox {
    /// Lexical guard + normalization: reject `..`, absolute paths, and empty input before touching the
    /// filesystem, dropping any `.` components. Returns the root joined with the normal components.
    fn join_checked(&self, rel: &str) -> Result<PathBuf, AgentError> {
        if rel.trim().is_empty() {
            return Err(AgentError::Sandbox("empty path".to_string()));
        }
        let mut clean = self.root.clone();
        for component in Path::new(rel).components() {
            match component {
                Component::Normal(segment) => clean.push(segment),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(AgentError::Sandbox(format!(
                        "path traversal ('..') is not allowed: {rel}"
                    )));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(AgentError::Sandbox(format!(
                        "absolute paths are not allowed: {rel}"
                    )));
                }
            }
        }
        Ok(clean)
    }

    fn assert_within(&self, real: &Path) -> Result<(), AgentError> {
        if !real.starts_with(&self.root) {
            return Err(AgentError::Sandbox(
                "path escapes the sandbox root".to_string(),
            ));
        }
        Ok(())
    }

    /// Refuse CRUD on files whose name matches a sensitive pattern (secrets, keys, credentials).
    /// The check is on the last path component only, so a directory named `.ssh` is not blocked
    /// (the files inside it are: `id_rsa`, `authorized_keys`, etc.).
    fn assert_not_sensitive(&self, real: &Path, display: &str) -> Result<(), AgentError> {
        if let Some(name) = real.file_name().and_then(|n| n.to_str())
            && let Some(glob) = self.sensitive.matches(name)
        {
            return Err(AgentError::Sandbox(format!(
                "path matches sensitive file pattern '{glob}': {display}"
            )));
        }
        Ok(())
    }

    /// Refuse any resolved path that lies inside a credential directory (`.ssh`/`.aws`/…) — the
    /// single chokepoint that covers the single-path tools (read/write/edit/delete/move), which the
    /// file-name guard alone misses for non-sensitive names like `~/.aws/config`. For an in-root path
    /// only the portion beyond the workspace root is inspected, so a workspace that itself sits under
    /// such a directory is not self-blocked; an out-of-root (absolute/`~`) target is inspected in full.
    fn assert_not_in_secret_dir(&self, real: &Path, display: &str) -> Result<(), AgentError> {
        let scoped = real.strip_prefix(&self.root).unwrap_or(real);
        if let Some(name) = self.secret_dir_component(scoped) {
            return Err(AgentError::Sandbox(format!(
                "path is inside the secret directory '{name}': {display}"
            )));
        }
        Ok(())
    }

    /// Refuse any path under the harness global private tree (`~/.kiri`). Unlike `SECRET_DIRS`, this
    /// is a **prefix** check against the resolved home harness dir — project-local `.kiri/` under a
    /// workspace that is not the home directory remains allowed (memory, project config).
    fn assert_not_under_harness_private(
        &self,
        real: &Path,
        display: &str,
    ) -> Result<(), AgentError> {
        if is_under_harness_private(real) {
            return Err(AgentError::Sandbox(format!(
                "path is inside the harness private directory '{HARNESS_PRIVATE_DIR}': {display}"
            )));
        }
        Ok(())
    }

    /// Refuse multi-component home secret stores (e.g. `~/.config/gh`) that cannot be expressed as a
    /// single `SECRET_DIRS` component without denying all of `.config`.
    fn assert_not_under_home_secret_subpaths(
        &self,
        real: &Path,
        display: &str,
    ) -> Result<(), AgentError> {
        if !is_under_home_secret_subpath(real) {
            return Ok(());
        }
        // Prefer a stable label for the error (first matching subpath).
        let Some(home_dir) = home() else {
            return Ok(());
        };
        for components in HOME_SECRET_SUBPATHS {
            if path_is_under_home_join(real, Some(home_dir.as_path()), components) {
                let label = components.join("/");
                return Err(AgentError::Sandbox(format!(
                    "path is inside the secret directory '{label}': {display}"
                )));
            }
        }
        Ok(())
    }
}

/// Whether `real` lies under the global harness private tree (`~/.kiri`). Used by resolve_* and by
/// recursive tools (`search`) so a home-rooted workspace cannot walk harness state.
pub(crate) fn is_under_harness_private(real: &Path) -> bool {
    path_is_under_home_join(real, home().as_deref(), &[HARNESS_PRIVATE_DIR])
}

/// Whether `real` lies under a multi-component home secret store (e.g. `~/.config/gh`).
pub(crate) fn is_under_home_secret_subpath(real: &Path) -> bool {
    let Some(home_dir) = home() else {
        return false;
    };
    HOME_SECRET_SUBPATHS
        .iter()
        .any(|components| path_is_under_home_join(real, Some(home_dir.as_path()), components))
}

/// Whether `real` equals or is a descendant of `home.join(components…)`.
///
/// Home is canonicalized first (it always exists) so the joined target shares the same path form as
/// `resolve_*` outputs (including the Windows `\\?\` verbatim prefix). Canonicalizing only the final
/// target fails when the leaf does not exist yet (`resolve_create`), and a plain `C:\…` prefix never
/// matches a `\\?\C:\…` resolved path.
fn path_is_under_home_join(real: &Path, home: Option<&Path>, components: &[&str]) -> bool {
    let Some(home) = home else {
        return false;
    };
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let mut target = home;
    for component in components {
        target.push(component);
    }
    // Prefer a fully-canonical target when every component exists (symlink-safe); otherwise keep the
    // lexical join under the canonical home so create-into-missing-dirs still matches.
    let target = std::fs::canonicalize(&target).unwrap_or(target);
    real == target.as_path() || real.starts_with(&target)
}

#[path = "sandbox_tests.rs"]
#[cfg(test)]
mod tests;
