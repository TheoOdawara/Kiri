use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::modules::tools::application::command_sandbox::{
    CommandSandbox, NetworkPolicy, SandboxPolicy,
};
use crate::modules::tools::application::sandbox::{CreateResolution, Sandbox};
#[cfg(test)]
use crate::modules::tools::infrastructure::confine::noop::NoConfinement;
use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;

/// Directory names that hold credentials/keys. Every path resolution refuses to operate *inside* one
/// of these, since the file-name sensitive guard matches files, not directories — without this,
/// `search` could recurse into `~/.ssh` and print `id_rsa` line by line, and `read_file` could read
/// `~/.aws/config` (a non-sensitive *name* in a secret *dir*). Exposed so `search`/`list_dir` can also
/// exclude these directories from their own recursion. Compared case-insensitively (macOS APFS).
pub(crate) const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"];

/// Confines every file operation to a canonicalized root directory, and refuses CRUD on files
/// whose name matches a sensitive pattern (secrets, keys, credentials). All file tools resolve
/// their path through this type; nothing else touches the filesystem with a raw, unvalidated path.
#[derive(Debug, Clone)]
pub struct FsSandbox {
    root: PathBuf,
    sensitive: SensitiveMatcher,
    /// OS-level confinement applied to every child process the tools spawn. `NoConfinement` on
    /// platforms without a facility (and `KIRI_SANDBOX=off`), the platform adapter otherwise.
    confiner: Arc<dyn CommandSandbox>,
    /// The base network stance for `run_command` (a dev-command allow-list may widen it per call).
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
        let target = if expanded.is_absolute() {
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

    /// The base network stance (the default for `run_command` before its dev-command allow-list).
    fn network(&self) -> NetworkPolicy {
        self.network
    }

    /// Build the per-call OS-confinement policy: writes confined to the workspace root plus the
    /// configured extras and any per-call `extra_rw` (e.g. an approved out-of-root target's directory).
    fn command_policy(&self, network: NetworkPolicy, extra_rw: &[&Path]) -> SandboxPolicy {
        let mut rw: Vec<PathBuf> = self.extra_rw.to_vec();
        rw.extend(extra_rw.iter().map(|path| path.to_path_buf()));
        SandboxPolicy {
            root: self.root.clone(),
            network,
            extra_ro: self.extra_ro.to_vec(),
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

    /// Whether a resolved absolute path lies outside the active workspace root. Used by the file tools
    /// to phrase the out-of-jail case and pick the working directory the command runs in.
    fn is_outside_root(&self, resolved: &Path) -> bool {
        !resolved.starts_with(&self.root)
    }

    /// The working directory a command should run in for `resolved`. Inside the jail every command runs
    /// at the workspace root. When the user has approved an out-of-jail target, the command runs at that
    /// target's nearest existing directory — the harness "moves" there for that one call and, since each
    /// call builds its own process, is back at the root for the next (no process-global `chdir`).
    fn exec_cwd_for(&self, resolved: &Path) -> PathBuf {
        if !self.is_outside_root(resolved) {
            return self.root.clone();
        }
        let mut dir = resolved;
        loop {
            if dir.is_dir() {
                return dir.to_path_buf();
            }
            match dir.parent() {
                // Don't let the walk climb out to a filesystem root like `/`: if no existing ancestor
                // is found within the target's own subtree, run at the workspace root instead, so a
                // command never executes at `/` or the user's home for a deep nonexistent target.
                Some(parent) if parent.parent().is_some() => dir = parent,
                _ => return self.root.clone(),
            }
        }
    }

    /// Resolve a path that must already exist (read/edit/overwrite/delete/list/search). A relative path
    /// resolves under the active root and is asserted to stay within it; an absolute path (or `~/…`) is
    /// resolved as given — allowed outside the root, since the CLI gates it with explicit confirmation.
    fn resolve_existing(&self, rel: &str) -> Result<PathBuf> {
        let expanded = expand_tilde(rel, home().as_deref());
        if expanded.is_absolute() {
            let real = std::fs::canonicalize(&expanded)
                .with_context(|| format!("path not found: {rel}"))?;
            self.assert_not_sensitive(&real, rel)?;
            self.assert_not_in_secret_dir(&real, rel)?;
            return Ok(real);
        }
        let candidate = self.join_checked(rel)?;
        let real =
            std::fs::canonicalize(&candidate).with_context(|| format!("path not found: {rel}"))?;
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
    fn resolve_create(&self, rel: &str) -> Result<CreateResolution> {
        let expanded = expand_tilde(rel, home().as_deref());
        let (candidate, confined) = if expanded.is_absolute() {
            // Absolute/tilde-expanded paths are user-approved out-of-root targets — not confined here;
            // see the security model above.
            (expanded, false)
        } else {
            (self.join_checked(rel)?, true)
        };
        if confined && candidate == self.root {
            bail!("path has no file name: {rel}");
        }

        let mut existing = candidate.as_path();
        let mut missing: Vec<&Path> = Vec::new();
        while !existing.exists() {
            missing.push(existing);
            existing = existing
                .parent()
                .with_context(|| format!("invalid path: {rel}"))?;
        }

        let real_existing = std::fs::canonicalize(existing)
            .with_context(|| format!("cannot resolve an existing ancestor of {rel}"))?;
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
                .with_context(|| format!("invalid path component in {rel}"))?;
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
    fn join_checked(&self, rel: &str) -> Result<PathBuf> {
        if rel.trim().is_empty() {
            bail!("empty path");
        }
        let mut clean = self.root.clone();
        for component in Path::new(rel).components() {
            match component {
                Component::Normal(segment) => clean.push(segment),
                Component::CurDir => {}
                Component::ParentDir => bail!("path traversal ('..') is not allowed: {rel}"),
                Component::RootDir | Component::Prefix(_) => {
                    bail!("absolute paths are not allowed: {rel}")
                }
            }
        }
        Ok(clean)
    }

    fn assert_within(&self, real: &Path) -> Result<()> {
        if !real.starts_with(&self.root) {
            bail!("path escapes the sandbox root");
        }
        Ok(())
    }

    /// Refuse CRUD on files whose name matches a sensitive pattern (secrets, keys, credentials).
    /// The check is on the last path component only, so a directory named `.ssh` is not blocked
    /// (the files inside it are: `id_rsa`, `authorized_keys`, etc.).
    fn assert_not_sensitive(&self, real: &Path, display: &str) -> Result<()> {
        if let Some(name) = real.file_name().and_then(|n| n.to_str())
            && let Some(glob) = self.sensitive.matches(name)
        {
            bail!("path matches sensitive file pattern '{glob}': {display}");
        }
        Ok(())
    }

    /// Refuse any resolved path that lies inside a credential directory (`.ssh`/`.aws`/…) — the
    /// single chokepoint that covers the single-path tools (read/write/edit/delete/move), which the
    /// file-name guard alone misses for non-sensitive names like `~/.aws/config`. For an in-root path
    /// only the portion beyond the workspace root is inspected, so a workspace that itself sits under
    /// such a directory is not self-blocked; an out-of-root (absolute/`~`) target is inspected in full.
    fn assert_not_in_secret_dir(&self, real: &Path, display: &str) -> Result<()> {
        let scoped = real.strip_prefix(&self.root).unwrap_or(real);
        if let Some(name) = self.secret_dir_component(scoped) {
            bail!("path is inside the secret directory '{name}': {display}");
        }
        Ok(())
    }
}

/// Expand a leading `~` (alone) or `~/…` to `home`; any other path is returned unchanged. `~user` is
/// intentionally not expanded. Pure, for testability.
fn expand_tilde(path: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if path == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = path.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Whether a tool path targets an explicit absolute location (after `~` expansion) — i.e. potentially
/// outside the active workspace. Used to pick the confirmation default (accept inside, decline outside).
/// The model emits Unix-style paths, so a leading `/` is treated as absolute on every platform
/// (`Path::is_absolute` would miss it on Windows, where a drive prefix is required).
pub(crate) fn is_absolute_target(path: &str) -> bool {
    path.starts_with('/') || expand_tilde(path, home().as_deref()).is_absolute()
}

/// The confirmation default for a tool path: accept inside the workspace, decline for an explicit
/// absolute/`~` target (potentially outside it). The single source of the in/out-of-workspace rule.
pub(crate) fn default_accept_for(path: &str) -> bool {
    !is_absolute_target(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sandbox(dir: &TempDir) -> FsSandbox {
        FsSandbox::new(dir.path(), SensitiveMatcher::empty()).unwrap()
    }

    fn guarded_sandbox(dir: &TempDir) -> FsSandbox {
        FsSandbox::new(
            dir.path(),
            SensitiveMatcher::new(&[".env", "id_rsa", "*.pem"]).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn new_rejects_missing_root() {
        assert!(FsSandbox::new("/nonexistent/t-cli/xyz-9999", SensitiveMatcher::empty()).is_err());
    }

    #[test]
    fn new_rejects_file_as_root() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, b"x").unwrap();
        assert!(FsSandbox::new(&file, SensitiveMatcher::empty()).is_err());
    }

    #[test]
    fn new_canonicalizes_root_to_absolute_existing_dir() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        assert!(sb.root().is_absolute());
        assert!(sb.root().is_dir());
    }

    #[test]
    fn rejects_parent_traversal() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        assert!(sb.resolve_existing("../etc").is_err());
        assert!(sb.resolve_existing("a/../../x").is_err());
        assert!(sb.resolve_create("../x.txt").is_err());
    }

    #[test]
    fn allows_absolute_path_outside_root() {
        let outside = TempDir::new().unwrap();
        let file = outside.path().join("f.txt");
        fs::write(&file, b"x").unwrap();

        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);

        let resolved = sb.resolve_existing(file.to_str().unwrap()).unwrap();
        assert_eq!(resolved, fs::canonicalize(&file).unwrap());

        let created = sb
            .resolve_create(outside.path().join("new.txt").to_str().unwrap())
            .unwrap();
        assert_eq!(
            created.target,
            fs::canonicalize(outside.path()).unwrap().join("new.txt")
        );
    }

    #[test]
    fn expand_tilde_expands_leading_home() {
        let home = Path::new("/home/u");
        assert_eq!(expand_tilde("~", Some(home)), PathBuf::from("/home/u"));
        assert_eq!(
            expand_tilde("~/dev/x", Some(home)),
            PathBuf::from("/home/u/dev/x")
        );
        assert_eq!(expand_tilde("dev/x", Some(home)), PathBuf::from("dev/x"));
        assert_eq!(expand_tilde("/abs", Some(home)), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("~/x", None), PathBuf::from("~/x"));
    }

    #[test]
    fn secret_dir_component_flags_credential_directories() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        assert_eq!(
            sb.secret_dir_component(Path::new("/home/u/.ssh/id_rsa")),
            Some(".ssh")
        );
        assert_eq!(
            sb.secret_dir_component(Path::new("/home/u/.aws/config")),
            Some(".aws")
        );
        assert_eq!(
            sb.secret_dir_component(Path::new("/home/u/project/src/main.rs")),
            None
        );
    }

    #[test]
    fn secret_dir_component_is_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        assert_eq!(
            sb.secret_dir_component(Path::new("/home/u/.SSH/id_rsa")),
            Some(".ssh")
        );
        assert_eq!(
            sb.secret_dir_component(Path::new("/home/u/.Docker/config.json")),
            Some(".docker")
        );
    }

    // The sensitive-file guard is the secrets chokepoint, yet every other test builds the sandbox with
    // an empty matcher. These lock that resolve_existing/resolve_create actually refuse a sensitive
    // name through a real matcher (the highest-value missing coverage the audit flagged).
    #[test]
    fn resolve_existing_refuses_a_sensitive_file() {
        let dir = TempDir::new().unwrap();
        let sb = guarded_sandbox(&dir);
        fs::write(dir.path().join(".env"), b"SECRET=1").unwrap();
        fs::write(dir.path().join("id_rsa"), b"key").unwrap();
        fs::write(dir.path().join("server.pem"), b"cert").unwrap();
        assert!(sb.resolve_existing(".env").is_err());
        assert!(sb.resolve_existing("id_rsa").is_err());
        assert!(sb.resolve_existing("server.pem").is_err());
    }

    #[test]
    fn resolve_create_refuses_a_sensitive_file() {
        let dir = TempDir::new().unwrap();
        let sb = guarded_sandbox(&dir);
        assert!(sb.resolve_create(".env").is_err());
        assert!(sb.resolve_create("sub/id_rsa").is_err());
    }

    #[test]
    fn resolve_create_refuses_an_out_of_root_sensitive_file() {
        let outside = TempDir::new().unwrap();
        let sb = guarded_sandbox(&TempDir::new().unwrap());
        let leak = outside.path().join("leak.pem");
        assert!(sb.resolve_create(leak.to_str().unwrap()).is_err());
    }

    // The single-path tools (read/write/edit/delete/move) resolve through these methods, so refusing a
    // path inside a credential directory here closes the gap the file-name guard misses (e.g. a
    // non-sensitive name like `config` living in `.aws`/`.ssh`/`.docker`).
    #[test]
    fn resolve_refuses_paths_inside_a_credential_dir() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        fs::create_dir(dir.path().join(".ssh")).unwrap();
        fs::write(dir.path().join(".ssh").join("config"), b"x").unwrap();
        assert!(sb.resolve_existing(".ssh/config").is_err());
        assert!(sb.resolve_create(".ssh/newkey").is_err());
    }

    #[test]
    fn resolve_refuses_out_of_root_credential_dir() {
        let outside = TempDir::new().unwrap();
        fs::create_dir(outside.path().join(".aws")).unwrap();
        fs::write(outside.path().join(".aws").join("region"), b"k").unwrap();
        let sb = sandbox(&TempDir::new().unwrap());
        let inside_creds = outside.path().join(".aws").join("region");
        assert!(sb.resolve_existing(inside_creds.to_str().unwrap()).is_err());
    }

    #[test]
    fn rejects_empty_path() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        assert!(sb.resolve_existing("").is_err());
        assert!(sb.resolve_existing("   ").is_err());
    }

    #[test]
    fn resolve_existing_finds_file_in_root() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        fs::write(sb.root().join("f.txt"), b"hi").unwrap();
        let real = sb.resolve_existing("f.txt").unwrap();
        assert!(real.starts_with(sb.root()));
        assert_eq!(real.file_name().unwrap(), "f.txt");
    }

    #[test]
    fn resolve_create_new_file_in_existing_dir_has_no_missing_dirs() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        let res = sb.resolve_create("new.txt").unwrap();
        assert!(res.missing_dirs.is_empty());
        assert_eq!(res.target, sb.root().join("new.txt"));
    }

    #[test]
    fn resolve_create_reports_missing_intermediate_dirs() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        let res = sb.resolve_create("a/b/c.txt").unwrap();
        assert_eq!(
            res.missing_dirs,
            vec![sb.root().join("a"), sb.root().join("a").join("b")]
        );
        assert_eq!(res.target, sb.root().join("a").join("b").join("c.txt"));
    }

    #[test]
    fn resolve_create_rejects_traversal_even_with_missing_parents() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        assert!(sb.resolve_create("../../x/y.txt").is_err());
    }

    #[test]
    fn resolve_create_rejects_root_only_path() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        assert!(sb.resolve_create(".").is_err());
    }

    #[test]
    fn exec_cwd_for_stays_at_root_inside_the_jail() {
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        fs::write(sb.root().join("f.txt"), b"x").unwrap();
        let inside = sb.resolve_existing("f.txt").unwrap();
        assert!(!sb.is_outside_root(&inside));
        assert_eq!(sb.exec_cwd_for(&inside), sb.root());
    }

    #[test]
    fn exec_cwd_for_uses_the_external_dir_outside_the_jail() {
        let outside = TempDir::new().unwrap();
        let file = outside.path().join("f.txt");
        fs::write(&file, b"x").unwrap();
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);

        let resolved = sb.resolve_existing(file.to_str().unwrap()).unwrap();
        assert!(sb.is_outside_root(&resolved));
        // The command runs in the target file's directory, not the workspace root.
        assert_eq!(
            sb.exec_cwd_for(&resolved),
            fs::canonicalize(outside.path()).unwrap()
        );
    }

    #[test]
    fn exec_cwd_for_falls_back_to_nearest_existing_ancestor() {
        let outside = TempDir::new().unwrap();
        let sb = sandbox(&TempDir::new().unwrap());
        // A deep, not-yet-created target outside the jail: cwd must be an existing directory.
        let target = sb
            .resolve_create(outside.path().join("a/b/c.txt").to_str().unwrap())
            .unwrap()
            .target;
        assert_eq!(
            sb.exec_cwd_for(&target),
            fs::canonicalize(outside.path()).unwrap()
        );
    }

    #[test]
    fn exec_cwd_for_never_escapes_to_the_filesystem_root() {
        let sb = sandbox(&TempDir::new().unwrap());
        // A nonexistent target directly under `/`: its only existing ancestor is `/` itself, which
        // must never become the working directory — fall back to the workspace root instead.
        let target = sb
            .resolve_create("/t-cli-nonexistent-top-zzz-9999/f.txt")
            .unwrap()
            .target;
        assert_eq!(sb.exec_cwd_for(&target), sb.root());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_rejects_symlink_out_of_root() {
        let outside = TempDir::new().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, b"top secret").unwrap();

        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        std::os::unix::fs::symlink(&secret, sb.root().join("link.txt")).unwrap();

        assert!(sb.resolve_existing("link.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_rejects_symlinked_parent_out_of_root() {
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("f.txt"), b"x").unwrap();

        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        std::os::unix::fs::symlink(outside.path(), sb.root().join("link")).unwrap();

        assert!(sb.resolve_existing("link/f.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_create_rejects_symlinked_ancestor_out_of_root() {
        let outside = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let sb = sandbox(&dir);
        std::os::unix::fs::symlink(outside.path(), sb.root().join("link")).unwrap();

        assert!(sb.resolve_create("link/new.txt").is_err());
    }
}
