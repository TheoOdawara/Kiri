use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::modules::tools::application::command_sandbox::{CommandSandbox, SandboxPolicy};
use crate::modules::tools::application::path::{expand_tilde, home, is_absolute_path};
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
            expanded
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
        let expanded = expand_tilde(rel, home().as_deref());
        if is_absolute_path(rel, &expanded) {
            let real = std::fs::canonicalize(&expanded)
                .map_err(|_| AgentError::Sandbox(format!("path not found: {rel}")))?;
            self.assert_not_sensitive(&real, rel)?;
            self.assert_not_in_secret_dir(&real, rel)?;
            return Ok(real);
        }
        let candidate = self.join_checked(rel)?;
        let real = std::fs::canonicalize(&candidate)
            .map_err(|_| AgentError::Sandbox(format!("path not found: {rel}")))?;
        self.assert_within(&real)?;
        self.assert_not_sensitive(&real, rel)?;
        self.assert_not_in_secret_dir(&real, rel)?;
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
        let expanded = expand_tilde(rel, home().as_deref());
        let (candidate, confined) = if is_absolute_path(rel, &expanded) {
            // Absolute/tilde-expanded paths are user-approved out-of-root targets — not confined here;
            // see the security model above.
            (expanded, false)
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
}

#[path = "sandbox_tests.rs"]
#[cfg(test)]
mod tests;
