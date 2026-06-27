use std::path::{Path, PathBuf};

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::sync::application::git::Git;
use crate::modules::sync::application::work_tree::SyncWorkTree;
use crate::modules::sync::domain::config_trust::risky_config_changes;
use crate::modules::sync::infrastructure::memory_ndjson;
use crate::shared::infra::config;
use crate::shared::kernel::error::AgentError;

type Result<T> = std::result::Result<T, AgentError>;

/// The files the sync work-tree holds. Secrets (`credentials.json`, keyring) are NEVER among them — only
/// the non-secret `config.toml` and the memory NDJSON are written into the tree.
const MEMORY_FILE: &str = "memory.ndjson";
const CONFIG_FILE: &str = "config.toml";
const GITIGNORE: &str = "# Kiri sync — never commit secrets or machine-local binary state\ncredentials.json\n*.db\n*.db-journal\n*.db-wal\nembeddings.json\n";

/// The branch the profile lives on, pinned so push and pull agree across machines.
const SYNC_BRANCH: &str = "main";

/// Orchestrates the portable-profile sync: it exports the non-secret config and the shared memory into a
/// dedicated git work-tree (`~/.kiri/sync`), and pushes/pulls it against a user-owned private repo. The
/// `Git` operations are a port, so this is testable without a real repo.
pub struct SyncService<'a> {
    git: &'a dyn Git,
    /// `~/.kiri` — the harness home; the sync work-tree lives at `<global_dir>/sync`.
    global_dir: PathBuf,
    /// The live global config file (`~/.kiri/config.toml`).
    config_path: PathBuf,
    /// The shared memory store, exported to / imported from NDJSON. Injected as a port so this
    /// application service never depends on the concrete SQLite adapter (the caller owns the store).
    memory: &'a dyn SharedMemory,
    /// Every work-tree/config filesystem touch, behind a port (like `git`/`memory`) so this use-case
    /// holds no raw filesystem calls — neither writes/reads nor presence checks.
    work_tree: &'a dyn SyncWorkTree,
}

impl<'a> SyncService<'a> {
    pub fn new(
        git: &'a dyn Git,
        global_dir: PathBuf,
        config_path: PathBuf,
        memory: &'a dyn SharedMemory,
        work_tree: &'a dyn SyncWorkTree,
    ) -> Self {
        Self {
            git,
            global_dir,
            config_path,
            memory,
            work_tree,
        }
    }

    fn sync_dir(&self) -> PathBuf {
        self.global_dir.join("sync")
    }

    /// Initialize the sync work-tree and point it at `remote_url`. Idempotent: re-running updates the
    /// remote URL rather than failing. The URL is validated before it reaches `git remote add` (which
    /// has no `--` end-of-options marker), so a hostile transport cannot smuggle command execution.
    pub async fn init(&self, remote_url: &str) -> Result<String> {
        self.validate_remote_url(remote_url).await?;
        let dir = self.sync_dir();
        self.work_tree.ensure_dir(&dir).await?;
        if !self.work_tree.exists(&dir.join(".git")).await? {
            self.git_ok(&["init"], &dir).await?;
            // Pin the branch name so push/pull agree regardless of the host's init.defaultBranch.
            // Best-effort: renaming an unborn branch can no-op on some git versions.
            let _ = self.git.run(&["branch", "-m", SYNC_BRANCH], &dir).await;
        }
        self.work_tree
            .write(&dir.join(".gitignore"), GITIGNORE)
            .await?;
        // Set the remote, replacing any existing one so re-init can repoint it.
        let _ = self.git.run(&["remote", "remove", "origin"], &dir).await;
        self.git_ok(&["remote", "add", "origin", remote_url], &dir)
            .await?;
        Ok(format!(
            "sync initialized at {} → {remote_url}",
            dir.display()
        ))
    }

    /// Export the profile, commit, and push. A no-op commit (nothing changed) is not an error.
    pub async fn push(&self) -> Result<String> {
        let dir = self.require_initialized().await?;
        let count = self.export_profile(&dir).await?;
        self.git_ok(&["add", "-A"], &dir).await?;
        // An empty commit fails; treat "nothing to commit" as success.
        let commit = self.git.run(&["commit", "-m", "kiri sync"], &dir).await?;
        if !commit.success
            && !commit.stdout.contains("nothing to commit")
            && !commit.stderr.contains("nothing to commit")
        {
            return Err(AgentError::Sync(format!(
                "git commit failed: {}",
                first_line(&commit.stderr, &commit.stdout)
            )));
        }
        let push = self
            .git
            .run(&["push", "-u", "origin", SYNC_BRANCH], &dir)
            .await?;
        if !push.success {
            return Err(AgentError::Sync(format!(
                "git push failed: {}",
                first_line(&push.stderr, &push.stdout)
            )));
        }
        Ok(format!("pushed {count} memory entries + config"))
    }

    /// Pull and merge: fetch the latest, merge memory last-write-wins, and apply config unless the change
    /// is risky (a provider base_url change or a sandbox downgrade) and `force` is not set.
    pub async fn pull(&self, force: bool) -> Result<String> {
        let dir = self.require_initialized().await?;
        // Fetch then hard-reset the work-tree to the remote. The work-tree holds only export artifacts
        // (NDJSON + a config copy), so resetting is safe: the live memory database is outside the tree and
        // is merged separately below (last-write-wins), and the live config is applied under a trust
        // check. This also handles a fresh clone whose local branch is still unborn, where `pull
        // --ff-only` fails.
        self.git_ok(&["fetch", "origin", SYNC_BRANCH], &dir).await?;
        self.git_ok(&["reset", "--hard", "FETCH_HEAD"], &dir)
            .await?;

        // Merge memory (always safe — last-write-wins, never destructive).
        let report = memory_ndjson::import(self.memory, &dir.join(MEMORY_FILE)).await?;

        // Apply config under the trust check. `read_to_string` returns `None` for an absent work-tree
        // config (no config in sync), collapsing the former presence-check + read pair.
        let incoming_config = dir.join(CONFIG_FILE);
        let config_note = match self.work_tree.read_to_string(&incoming_config).await? {
            Some(incoming) => {
                // Refuse a config that is valid TOML but invalid against the real schema, regardless of
                // `force` — writing it would brick the next boot when it fails to deserialize.
                if let Err(error) = config::validate_config_str(&incoming) {
                    format!("config NOT applied ({error})")
                } else {
                    // Establish the trusted baseline. A genuinely absent current config is an empty
                    // baseline (first pull, `Ok(None)`); a present-but-unreadable one cannot be trusted
                    // (a read error), so we cannot prove the change is safe and require `--force`.
                    let current = match self.work_tree.read_to_string(&self.config_path).await {
                        Ok(Some(text)) => Some(text),
                        Ok(None) => Some(String::new()),
                        Err(_) => None,
                    };
                    let risks = match &current {
                        Some(text) => risky_config_changes(text, &incoming),
                        None => vec![
                            "current config is unreadable; cannot verify the change is safe"
                                .to_string(),
                        ],
                    };
                    if risks.is_empty() || force {
                        self.work_tree
                            .write_atomic(&self.config_path, &incoming)
                            .await?;
                        if risks.is_empty() {
                            "config applied".to_string()
                        } else {
                            format!("config applied with --force despite: {}", risks.join("; "))
                        }
                    } else {
                        format!(
                            "config NOT applied (re-run with --force to accept): {}",
                            risks.join("; ")
                        )
                    }
                }
            }
            None => "no config in sync".to_string(),
        };

        Ok(format!(
            "merged {} memory entries ({} kept local); {config_note}",
            report.merged, report.skipped
        ))
    }

    /// The git status of the sync work-tree.
    pub async fn status(&self) -> Result<String> {
        let dir = self.require_initialized().await?;
        let output = self
            .git
            .run(&["status", "--short", "--branch"], &dir)
            .await?;
        if !output.success {
            return Err(AgentError::Sync(format!(
                "git status failed: {}",
                first_line(&output.stderr, &output.stdout)
            )));
        }
        // `--branch` always prints a leading `## <branch>...` header line, so emptiness is never the
        // signal — the tree is clean iff there are no porcelain entries beyond that header.
        let dirty = output
            .stdout
            .lines()
            .any(|line| !line.starts_with("##") && !line.trim().is_empty());
        Ok(if dirty {
            output.stdout
        } else {
            "sync clean".to_string()
        })
    }

    /// Write the non-secret profile into the work-tree, returning the exported memory-entry count.
    async fn export_profile(&self, dir: &Path) -> Result<usize> {
        self.work_tree
            .write(&dir.join(".gitignore"), GITIGNORE)
            .await?;
        if self.work_tree.exists(&self.config_path).await? {
            self.work_tree
                .copy(&self.config_path, &dir.join(CONFIG_FILE))
                .await?;
        }
        memory_ndjson::export(self.memory, &dir.join(MEMORY_FILE)).await
    }

    async fn require_initialized(&self) -> Result<PathBuf> {
        let dir = self.sync_dir();
        if !self.work_tree.exists(&dir.join(".git")).await? {
            return Err(AgentError::Sync(
                "sync not initialized — run `kiri sync init <repo-url>` first".to_string(),
            ));
        }
        Ok(dir)
    }

    async fn git_ok(&self, args: &[&str], cwd: &Path) -> Result<()> {
        let output = self.git.run(args, cwd).await?;
        if output.success {
            Ok(())
        } else {
            Err(AgentError::Sync(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or(""),
                first_line(&output.stderr, &output.stdout)
            )))
        }
    }

    /// Validate a `kiri sync init` remote URL before it reaches `git remote add`. `git` treats some
    /// "URLs" as code-execution transports (`ext::sh -c …`, and any other `<helper>::…` remote helper) or
    /// as options (a leading `-`, e.g. `-oProxyCommand=…`), either of which turns an attacker-controlled
    /// URL into a command-injection vector. `git remote add` takes no `--` end-of-options marker, so this
    /// validation is the defense — a positive ALLOWLIST: accept only the ordinary transports
    /// (https/http/ssh, the scp-like `user@host:path`, and a local filesystem path) and reject everything
    /// else, so every `::` remote-helper transport (in any case) and every option-like input is rejected.
    async fn validate_remote_url(&self, url: &str) -> Result<()> {
        let url = url.trim();
        if url.is_empty() {
            return Err(AgentError::Sync("sync remote URL is empty".to_string()));
        }
        if self.is_allowed_remote_url(url).await? {
            Ok(())
        } else {
            Err(AgentError::Sync(format!(
                "refusing remote URL outside the allowed set (https/http/ssh, user@host:path, or a local path): {url}"
            )))
        }
    }

    /// The positive allowlist behind `validate_remote_url`: an ordinary scheme transport, a scp-like
    /// `user@host:path`, or a local filesystem path (absolute, or an existing relative one — the only
    /// branch that touches the filesystem, routed through the work-tree port). Any candidate containing
    /// `::` is rejected first, so a relative path that exists yet git would parse as a `<helper>::`
    /// remote-helper transport (e.g. a file literally named `evil::payload`) cannot slip through.
    async fn is_allowed_remote_url(&self, url: &str) -> Result<bool> {
        const SCHEMES: [&str; 3] = ["https://", "http://", "ssh://"];
        if SCHEMES.iter().any(|scheme| url.starts_with(scheme)) {
            return Ok(true);
        }
        if is_scp_like(url) {
            return Ok(true);
        }
        // `::` would let git dispatch a remote helper; never accept it through the local-path arm.
        if url.contains("::") {
            return Ok(false);
        }
        let path = Path::new(url);
        Ok(path.is_absolute() || self.work_tree.exists(path).await?)
    }
}

/// scp-like git syntax `user@host:path`: the text before the first `:` is `user@host`, where `user` and
/// `host` each start with an alphanumeric and otherwise hold only `[A-Za-z0-9._-]`. Requiring an
/// alphanumeric first char keeps an option (a leading `-`) from masquerading as a `user` segment.
fn is_scp_like(url: &str) -> bool {
    let Some((authority, _path)) = url.split_once(':') else {
        return false;
    };
    let Some((user, host)) = authority.split_once('@') else {
        return false;
    };
    is_host_segment(user) && is_host_segment(host)
}

fn is_host_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {
            chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        }
        _ => false,
    }
}

/// The first non-empty line of stderr, falling back to stdout, for a compact error message.
fn first_line(stderr: &str, stdout: &str) -> String {
    let pick = if stderr.is_empty() { stdout } else { stderr };
    pick.lines().next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
    use crate::modules::sync::application::git::GitOutput;
    use crate::modules::sync::infrastructure::fs_work_tree::FsSyncWorkTree;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // The sync application layer must orchestrate ports only; its non-test slice holds no raw filesystem
    // I/O (the `tokio::fs` calls *and* the `.exists()` reads now live behind the `SyncWorkTree` adapter).
    #[test]
    fn sync_service_has_no_inline_fs() {
        let source = include_str!("sync_service.rs");
        // Scope the guard to the slice before the test module (which legitimately uses std::fs over a
        // TempDir). Build needles by concatenation so this guard's own literals do not self-match.
        let head = source
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a pre-test slice");
        for needle in [
            concat!("tokio", "::fs"),
            concat!("use tokio", "::fs"),
            concat!(".", "exists()"),
            concat!("std", "::fs"),
            concat!("fs", "::"),
        ] {
            assert!(
                !head.contains(needle),
                "sync_service.rs (non-test) must not contain inline fs: found {needle:?}"
            );
        }
    }

    /// A `Git` double that records the commands it was asked to run and always succeeds. It creates the
    /// `.git` marker on `init`, and on `reset` (which `pull` runs after `fetch`) it materializes the
    /// configured fixture files into the work-tree, standing in for the remote's contents.
    struct FakeGit {
        calls: Mutex<Vec<String>>,
        config_fixture: Option<String>,
        memory_fixture: Option<String>,
    }

    impl FakeGit {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                config_fixture: None,
                memory_fixture: None,
            }
        }

        fn with_fixtures(config: Option<&str>, memory: Option<&str>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                config_fixture: config.map(String::from),
                memory_fixture: memory.map(String::from),
            }
        }
    }

    #[async_trait]
    impl Git for FakeGit {
        async fn run(&self, args: &[&str], cwd: &Path) -> Result<GitOutput> {
            self.calls.lock().unwrap().push(args.join(" "));
            if args.first() == Some(&"init") {
                std::fs::create_dir_all(cwd.join(".git")).unwrap();
            }
            if args.first() == Some(&"reset") {
                if let Some(config) = &self.config_fixture {
                    std::fs::write(cwd.join(CONFIG_FILE), config).unwrap();
                }
                if let Some(memory) = &self.memory_fixture {
                    std::fs::write(cwd.join(MEMORY_FILE), memory).unwrap();
                }
            }
            Ok(GitOutput {
                stdout: String::new(),
                stderr: String::new(),
                success: true,
            })
        }
    }

    /// A complete, schema-valid provider profile (the loader requires kind/base_url/model/auth;
    /// `ProviderKind::OpenAiCompatible` serializes kebab-case as `open-ai-compatible`).
    fn provider_toml(id: &str, base_url: &str) -> String {
        format!(
            "[providers.{id}]\nkind = \"open-ai-compatible\"\nbase_url = \"{base_url}\"\nmodel = \"m\"\nauth = \"api-key\"\n"
        )
    }

    #[tokio::test]
    async fn init_then_push_writes_profile_and_runs_git() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().to_path_buf();
        let config = global.join("config.toml");
        std::fs::write(&config, "[providers.nvidia]\nbase_url = \"https://x/v1\"\n").unwrap();
        // A real (empty) shared store so export has a valid port to read.
        let mem = SqliteSharedMemory::new(global.join("memory").join("shared.db")).unwrap();
        mem.init().await.unwrap();
        let git = FakeGit::new();
        let work_tree = FsSyncWorkTree;
        let service = SyncService::new(&git, global.clone(), config, &mem, &work_tree);

        service.init("git@example:me/profile.git").await.unwrap();
        service.push().await.unwrap();

        let sync_dir = global.join("sync");
        assert!(sync_dir.join("memory.ndjson").exists());
        assert!(sync_dir.join("config.toml").exists());
        assert!(sync_dir.join(".gitignore").exists());
        // The secret file must never be copied into the work-tree.
        assert!(!sync_dir.join("credentials.json").exists());
        let calls = git.calls.lock().unwrap();
        assert!(calls.iter().any(|c| c.starts_with("push")));
    }

    #[tokio::test]
    async fn push_before_init_errors() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().to_path_buf();
        let mem = SqliteSharedMemory::in_memory().unwrap();
        let git = FakeGit::new();
        let work_tree = FsSyncWorkTree;
        let service = SyncService::new(
            &git,
            global.clone(),
            global.join("config.toml"),
            &mem,
            &work_tree,
        );
        assert!(service.push().await.is_err());
    }

    /// Build a minimal service for the URL-validation tests (the validator is now a method that routes
    /// its local-path existence check through the work-tree port). The paths are placeholders — these
    /// tests never touch the work-tree beyond the allowlist's existence probe.
    fn url_service<'a>(
        git: &'a FakeGit,
        memory: &'a dyn SharedMemory,
        work_tree: &'a FsSyncWorkTree,
    ) -> SyncService<'a> {
        SyncService::new(
            git,
            PathBuf::from("/kiri-url-test"),
            PathBuf::from("/kiri-url-test/config.toml"),
            memory,
            work_tree,
        )
    }

    #[tokio::test]
    async fn validate_remote_url_rejects_ext_and_dash() {
        let git = FakeGit::new();
        let mem = SqliteSharedMemory::in_memory().unwrap();
        let work_tree = FsSyncWorkTree;
        let s = url_service(&git, &mem, &work_tree);
        assert!(
            s.validate_remote_url("ext::sh -c 'rm -rf ~'")
                .await
                .is_err()
        );
        assert!(s.validate_remote_url("fab::evil").await.is_err());
        // A case variant and any other `<helper>::` transport must fall out of the allowlist too.
        assert!(
            s.validate_remote_url("EXT::sh -c 'rm -rf ~'")
                .await
                .is_err()
        );
        assert!(s.validate_remote_url("fd::evil").await.is_err());
        assert!(s.validate_remote_url("-oProxyCommand=evil").await.is_err());
        assert!(s.validate_remote_url("file://-evil").await.is_err());
        assert!(s.validate_remote_url("   ").await.is_err());
        // `::` is rejected even on an otherwise-accepted absolute path, so a name git would dispatch as a
        // `<helper>::` transport cannot slip through the local-path arm (locks the pre-existence guard).
        assert!(s.validate_remote_url("/tmp/evil::payload").await.is_err());
    }

    #[tokio::test]
    async fn validate_remote_url_accepts_https_ssh_and_local() {
        let git = FakeGit::new();
        let mem = SqliteSharedMemory::in_memory().unwrap();
        let work_tree = FsSyncWorkTree;
        let s = url_service(&git, &mem, &work_tree);
        assert!(
            s.validate_remote_url("https://github.com/me/profile.git")
                .await
                .is_ok()
        );
        assert!(
            s.validate_remote_url("http://example.test/p.git")
                .await
                .is_ok()
        );
        assert!(
            s.validate_remote_url("ssh://git@host/me/profile.git")
                .await
                .is_ok()
        );
        assert!(
            s.validate_remote_url("git@github.com:me/profile.git")
                .await
                .is_ok()
        );
        assert!(
            s.validate_remote_url("/home/me/profiles/p.git")
                .await
                .is_ok()
        );
    }

    // End-to-end pull: FakeGit's `reset` materializes the remote's config + memory into the work-tree,
    // exercising the apply/refuse decision and the --force override on the security-weighted path.
    fn pull_service<'a>(
        git: &'a FakeGit,
        memory: &'a dyn SharedMemory,
        work_tree: &'a FsSyncWorkTree,
        global: &Path,
        current_config: &str,
    ) -> (SyncService<'a>, PathBuf) {
        let config = global.join("config.toml");
        std::fs::write(&config, current_config).unwrap();
        let service =
            SyncService::new(git, global.to_path_buf(), config.clone(), memory, work_tree);
        (service, config)
    }

    #[tokio::test]
    async fn pull_refuses_a_risky_config_without_force() {
        let dir = TempDir::new().unwrap();
        let current = format!(
            "active_provider = \"nvidia\"\n{}",
            provider_toml("nvidia", "https://nvidia/v1")
        );
        let incoming = format!(
            "active_provider = \"evil\"\n{}{}",
            provider_toml("nvidia", "https://nvidia/v1"),
            provider_toml("evil", "https://attacker/v1")
        );
        let git = FakeGit::with_fixtures(Some(&incoming), Some(""));
        let mem = SqliteSharedMemory::in_memory().unwrap();
        mem.init().await.unwrap();
        let work_tree = FsSyncWorkTree;
        let (service, config) = pull_service(&git, &mem, &work_tree, dir.path(), &current);
        service.init("git@example:me/p.git").await.unwrap();

        let summary = service.pull(false).await.unwrap();
        assert!(summary.contains("NOT applied"), "{summary}");
        let after = std::fs::read_to_string(&config).unwrap();
        assert!(
            after.contains("nvidia") && !after.contains("attacker"),
            "the trusted config must be untouched: {after}"
        );
    }

    #[tokio::test]
    async fn pull_applies_a_risky_config_with_force() {
        let dir = TempDir::new().unwrap();
        let current = format!(
            "active_provider = \"nvidia\"\n{}",
            provider_toml("nvidia", "https://nvidia/v1")
        );
        let incoming = format!(
            "active_provider = \"evil\"\n{}{}",
            provider_toml("nvidia", "https://nvidia/v1"),
            provider_toml("evil", "https://attacker/v1")
        );
        let git = FakeGit::with_fixtures(Some(&incoming), Some(""));
        let mem = SqliteSharedMemory::in_memory().unwrap();
        mem.init().await.unwrap();
        let work_tree = FsSyncWorkTree;
        let (service, config) = pull_service(&git, &mem, &work_tree, dir.path(), &current);
        service.init("git@example:me/p.git").await.unwrap();

        let summary = service.pull(true).await.unwrap();
        assert!(summary.contains("--force"), "{summary}");
        let after = std::fs::read_to_string(&config).unwrap();
        assert!(
            after.contains("attacker"),
            "forced config must be applied: {after}"
        );
    }

    #[tokio::test]
    async fn pull_applies_a_safe_config() {
        let dir = TempDir::new().unwrap();
        let config_text = provider_toml("nvidia", "https://nvidia/v1");
        let git = FakeGit::with_fixtures(Some(&config_text), Some(""));
        let mem = SqliteSharedMemory::in_memory().unwrap();
        mem.init().await.unwrap();
        let work_tree = FsSyncWorkTree;
        let (service, config) = pull_service(&git, &mem, &work_tree, dir.path(), &config_text);
        service.init("git@example:me/p.git").await.unwrap();

        let summary = service.pull(false).await.unwrap();
        assert!(summary.contains("config applied"), "{summary}");
        assert!(std::fs::read_to_string(&config).unwrap().contains("nvidia"));
    }

    #[tokio::test]
    async fn pull_refuses_a_schema_invalid_config() {
        let dir = TempDir::new().unwrap();
        let git = FakeGit::with_fixtures(Some("effort = \"bogus\"\n"), Some(""));
        let mem = SqliteSharedMemory::in_memory().unwrap();
        mem.init().await.unwrap();
        let work_tree = FsSyncWorkTree;
        let (service, _config) = pull_service(
            &git,
            &mem,
            &work_tree,
            dir.path(),
            &provider_toml("nvidia", "https://x/v1"),
        );
        service.init("git@example:me/p.git").await.unwrap();

        let summary = service.pull(false).await.unwrap();
        assert!(summary.contains("NOT applied"), "{summary}");
    }
}
