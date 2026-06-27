use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::modules::tools::application::command_sandbox::{
    CommandSandbox, NetworkPolicy, SandboxPolicy,
};

/// The resolved target of a create operation, plus any parent directories that do not yet exist (in
/// shallow-first order) and would have to be created for the write to succeed. Pure data, so it lives
/// beside the port that produces it rather than in the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateResolution {
    pub target: PathBuf,
    pub missing_dirs: Vec<PathBuf>,
}

/// Port: the filesystem confinement boundary every tool resolves its paths through. The adapter
/// (`infrastructure::sandbox::FsSandbox`) owns the actual I/O — canonicalizing the root, checking
/// existence, refusing sensitive files and credential directories — so the application layer (the
/// `Tool` trait, the registry, `AgentLoop::run`) depends only on this capability, never on the
/// concrete type. Object-safe on purpose: tools receive `&dyn Sandbox`.
///
/// Construction (`with_confinement`) and the relocating `/cd` operation (which returns `Self`) stay
/// inherent on the concrete adapter — they are wiring/runtime concerns, not part of the consumed
/// capability, and `relocated` would break object-safety.
pub trait Sandbox {
    /// The canonical workspace root every confined operation is measured against.
    fn root(&self) -> &Path;

    /// Resolve a path that must already exist (read/edit/overwrite/delete/list/search), refusing
    /// traversal, sensitive names, and credential directories. Absolute/`~` targets are allowed but
    /// the engine gates them with explicit confirmation.
    fn resolve_existing(&self, rel: &str) -> Result<PathBuf>;

    /// Resolve a path for creation: the target need not exist and missing intermediate directories are
    /// reported, with the same sensitive-name and credential-directory guards applied.
    fn resolve_create(&self, rel: &str) -> Result<CreateResolution>;

    /// The working directory a command should run in for `resolved`: the root inside the jail, or the
    /// target's nearest existing directory for an approved out-of-root target.
    fn exec_cwd_for(&self, resolved: &Path) -> PathBuf;

    /// Whether a resolved absolute path lies outside the active workspace root.
    fn is_outside_root(&self, resolved: &Path) -> bool;

    /// The name of a secret directory (`.ssh`, `.aws`, …) that `real` lies within, if any, so the
    /// recursive tools can refuse to poke inside a credential store.
    fn secret_dir_component(&self, real: &Path) -> Option<&'static str>;

    /// Build the per-call OS-confinement policy. Writes are confined to the workspace root plus the
    /// configured extras and any per-call `extra_rw`; `extra_ro` grants per-call read access only. A
    /// read-only tool passes its cwd as `extra_ro` (least privilege — never a write grant), a mutating
    /// tool passes it as `extra_rw` (e.g. an approved out-of-root target's directory).
    fn command_policy(
        &self,
        network: NetworkPolicy,
        extra_ro: &[&Path],
        extra_rw: &[&Path],
    ) -> SandboxPolicy;

    /// The OS-confinement adapter every tool wraps its child process with before spawning.
    fn confiner(&self) -> &dyn CommandSandbox;

    /// The base network stance (the default for `run_command` before its dev-command allow-list).
    fn network(&self) -> NetworkPolicy;

    /// Whether a bare file name matches a sensitive pattern (secrets, keys, credentials). Exposed as a
    /// capability so the matcher type itself never leaks into the application layer.
    fn is_sensitive_name(&self, name: &str) -> bool;
}
