use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Confines every file operation to a canonicalized root directory. All file tools resolve their path
/// through this type; nothing else touches the filesystem with a raw, unvalidated path.
#[derive(Debug, Clone)]
pub struct Sandbox {
    root: PathBuf,
}

/// The resolved target of a create operation, plus any parent directories that do not yet exist (in
/// shallow-first order) and would have to be created for the write to succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateResolution {
    pub target: PathBuf,
    pub missing_dirs: Vec<PathBuf>,
}

impl Sandbox {
    /// Canonicalize the root once. Fails if it does not exist or is not a directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let canonical = std::fs::canonicalize(root)
            .with_context(|| format!("sandbox root {} does not exist", root.display()))?;
        if !canonical.is_dir() {
            bail!("sandbox root {} is not a directory", canonical.display());
        }
        Ok(Self { root: canonical })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a path that must already exist (read/edit/overwrite/delete/list/search). Resolves
    /// symlinks via `canonicalize`, then asserts the real path stays under the root.
    pub fn resolve_existing(&self, rel: &str) -> Result<PathBuf> {
        let candidate = self.join_checked(rel)?;
        let real =
            std::fs::canonicalize(&candidate).with_context(|| format!("path not found: {rel}"))?;
        self.assert_within(&real)?;
        Ok(real)
    }

    /// Resolve a path for creation. The target need not exist and intermediate directories may be
    /// missing. The deepest existing ancestor is canonicalized and asserted within the root, so the
    /// remaining (lexically clean) components appended onto it cannot escape.
    pub fn resolve_create(&self, rel: &str) -> Result<CreateResolution> {
        let candidate = self.join_checked(rel)?;
        if candidate == self.root {
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
        self.assert_within(&real_existing)?;

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

        Ok(CreateResolution {
            target: real_target,
            missing_dirs,
        })
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A unique temp directory, removed on drop. Avoids a dev-dependency for the same effect.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            let pid = std::process::id();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            path.push(format!("t-cli-{tag}-{pid}-{n}"));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sandbox(dir: &TempDir) -> Sandbox {
        Sandbox::new(&dir.path).unwrap()
    }

    #[test]
    fn new_rejects_missing_root() {
        assert!(Sandbox::new("/nonexistent/t-cli/xyz-9999").is_err());
    }

    #[test]
    fn new_rejects_file_as_root() {
        let dir = TempDir::new("file-root");
        let file = dir.path.join("f.txt");
        fs::write(&file, b"x").unwrap();
        assert!(Sandbox::new(&file).is_err());
    }

    #[test]
    fn new_canonicalizes_root_to_absolute_existing_dir() {
        let dir = TempDir::new("canon");
        let sb = sandbox(&dir);
        assert!(sb.root().is_absolute());
        assert!(sb.root().is_dir());
    }

    #[test]
    fn rejects_parent_traversal() {
        let dir = TempDir::new("traversal");
        let sb = sandbox(&dir);
        assert!(sb.resolve_existing("../etc").is_err());
        assert!(sb.resolve_existing("a/../../x").is_err());
        assert!(sb.resolve_create("../x.txt").is_err());
    }

    #[test]
    fn rejects_absolute_path() {
        let dir = TempDir::new("absolute");
        let sb = sandbox(&dir);
        assert!(sb.resolve_existing("/etc/passwd").is_err());
        assert!(sb.resolve_create("/tmp/x.txt").is_err());
    }

    #[test]
    fn rejects_empty_path() {
        let dir = TempDir::new("empty");
        let sb = sandbox(&dir);
        assert!(sb.resolve_existing("").is_err());
        assert!(sb.resolve_existing("   ").is_err());
    }

    #[test]
    fn resolve_existing_finds_file_in_root() {
        let dir = TempDir::new("find");
        let sb = sandbox(&dir);
        fs::write(sb.root().join("f.txt"), b"hi").unwrap();
        let real = sb.resolve_existing("f.txt").unwrap();
        assert!(real.starts_with(sb.root()));
        assert_eq!(real.file_name().unwrap(), "f.txt");
    }

    #[test]
    fn resolve_create_new_file_in_existing_dir_has_no_missing_dirs() {
        let dir = TempDir::new("create-flat");
        let sb = sandbox(&dir);
        let res = sb.resolve_create("new.txt").unwrap();
        assert!(res.missing_dirs.is_empty());
        assert_eq!(res.target, sb.root().join("new.txt"));
    }

    #[test]
    fn resolve_create_reports_missing_intermediate_dirs() {
        let dir = TempDir::new("create-nested");
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
        let dir = TempDir::new("create-traversal");
        let sb = sandbox(&dir);
        assert!(sb.resolve_create("../../x/y.txt").is_err());
    }

    #[test]
    fn resolve_create_rejects_root_only_path() {
        let dir = TempDir::new("create-dot");
        let sb = sandbox(&dir);
        assert!(sb.resolve_create(".").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_rejects_symlink_out_of_root() {
        let outside = TempDir::new("outside-target");
        let secret = outside.path.join("secret.txt");
        fs::write(&secret, b"top secret").unwrap();

        let dir = TempDir::new("inside-symlink");
        let sb = sandbox(&dir);
        std::os::unix::fs::symlink(&secret, sb.root().join("link.txt")).unwrap();

        assert!(sb.resolve_existing("link.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_rejects_symlinked_parent_out_of_root() {
        let outside = TempDir::new("outside-dir");
        fs::write(outside.path.join("f.txt"), b"x").unwrap();

        let dir = TempDir::new("inside-parent");
        let sb = sandbox(&dir);
        std::os::unix::fs::symlink(&outside.path, sb.root().join("link")).unwrap();

        assert!(sb.resolve_existing("link/f.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_create_rejects_symlinked_ancestor_out_of_root() {
        let outside = TempDir::new("outside-create");
        let dir = TempDir::new("inside-create");
        let sb = sandbox(&dir);
        std::os::unix::fs::symlink(&outside.path, sb.root().join("link")).unwrap();

        assert!(sb.resolve_create("link/new.txt").is_err());
    }
}
