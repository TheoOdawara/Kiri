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

// Lock the wiring: the resolution guard (`secret_dir_component`) must flag every directory name in
// the single-sourced `secret_paths::SECRET_DIRS`, so a future edit to the shared list cannot
// silently bypass the guard (and the Seatbelt layer that shares the same source).
#[test]
fn secret_paths_dirs_match_resolution_guard() {
    use crate::modules::tools::infrastructure::secret_paths;
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    for name in secret_paths::SECRET_DIRS {
        let path = PathBuf::from("/home/u").join(name).join("file");
        assert_eq!(
            sb.secret_dir_component(&path),
            Some(*name),
            "secret_dir_component must flag the single-sourced dir '{name}'"
        );
    }
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

// Least privilege: a read-only tool passes its cwd as a read extra (never a write grant), and a
// mutating tool passes its cwd as a write extra. The policy must reflect exactly that split.
#[test]
fn read_only_policy_has_no_write_extra() {
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    let cwd = sb.root().to_path_buf();
    let policy = sb.command_policy(NetworkPolicy::Deny, &[&cwd], &[]);
    assert!(
        policy.extra_rw.is_empty(),
        "a read-only tool must grant no write extra"
    );
    assert!(
        policy.extra_ro.contains(&cwd),
        "the read cwd must be a read extra"
    );
}

#[test]
fn mutating_policy_has_write_extra() {
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    let cwd = sb.root().to_path_buf();
    let policy = sb.command_policy(NetworkPolicy::Deny, &[], &[&cwd]);
    assert!(
        policy.extra_ro.is_empty(),
        "a mutating tool grants no read extra here"
    );
    assert!(
        policy.extra_rw.contains(&cwd),
        "the write cwd must be a write extra"
    );
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

// The port contract is the *typed* `AgentError::Sandbox`, not a stringified anyhow. These lock the
// refusal type at every guard so a future `?`-propagating caller keeps the typed contract.
#[test]
fn resolve_existing_traversal_yields_agenterror_sandbox() {
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    assert!(matches!(
        sb.resolve_existing("../etc"),
        Err(AgentError::Sandbox(_))
    ));
}

#[test]
fn resolve_create_sensitive_yields_agenterror_sandbox() {
    let dir = TempDir::new().unwrap();
    let sb = guarded_sandbox(&dir);
    assert!(matches!(
        sb.resolve_create(".env"),
        Err(AgentError::Sandbox(_))
    ));
}

#[test]
fn resolve_existing_credential_dir_yields_agenterror_sandbox() {
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    fs::create_dir(dir.path().join(".ssh")).unwrap();
    fs::write(dir.path().join(".ssh").join("config"), b"x").unwrap();
    assert!(matches!(
        sb.resolve_existing(".ssh/config"),
        Err(AgentError::Sandbox(_))
    ));
}

#[test]
fn resolve_existing_missing_yields_agenterror_sandbox() {
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    assert!(matches!(
        sb.resolve_existing("does-not-exist.txt"),
        Err(AgentError::Sandbox(_))
    ));
}

// Characterization lock for the rendered refusal text: the typed `AgentError::Sandbox` surfaces
// path-resolution refusals with the same "sandbox error: " prefix as confinement-setup failures — a
// deliberate, consistent surface the model/transcript sees (it replaced the bare anyhow message).
#[test]
fn resolve_refusal_renders_with_sandbox_error_prefix() {
    let dir = TempDir::new().unwrap();
    let sb = sandbox(&dir);
    let rendered = sb.resolve_existing("../escape").unwrap_err().to_string();
    assert!(
        rendered.starts_with("sandbox error: "),
        "expected the blessed sandbox-error prefix, got {rendered:?}"
    );
}

// Compile-asserting regression lock: the port methods expose the typed signature, not anyhow.
#[test]
fn sandbox_port_error_type_is_agenterror() {
    let _: fn(&FsSandbox, &str) -> Result<PathBuf, AgentError> = FsSandbox::resolve_existing;
    let _: fn(&FsSandbox, &str) -> Result<CreateResolution, AgentError> = FsSandbox::resolve_create;
}
