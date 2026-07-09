use std::path::{Path, PathBuf};

use crate::modules::sync::application::git::Git;
use crate::modules::sync::application::memory_exchange::MemoryExchange;
use crate::modules::sync::application::work_tree::SyncWorkTree;
use crate::modules::sync::domain::config_trust::risky_config_changes;
use crate::shared::infra::config;
use crate::shared::kernel::error::{AgentError, AgentResult};

/// The only files the work-tree ever holds. `credentials.json` is never among them.
const MEMORY_FILE: &str = "memory.ndjson";
const CONFIG_FILE: &str = "config.toml";
const GITIGNORE: &str = "# Kiri sync — never commit secrets or machine-local binary state\ncredentials.json\n*.db\n*.db-journal\n*.db-wal\nembeddings.json\n";

/// Pinned so push and pull agree across machines regardless of the host's `init.defaultBranch`.
const SYNC_BRANCH: &str = "main";

pub struct SyncService<'a> {
    git: &'a dyn Git,
    /// `~/.kiri`; the work-tree lives at `<global_dir>/sync`.
    global_dir: PathBuf,
    config_path: PathBuf,
    exchange: &'a dyn MemoryExchange,
    work_tree: &'a dyn SyncWorkTree,
}

impl<'a> SyncService<'a> {
    pub fn new(
        git: &'a dyn Git,
        global_dir: PathBuf,
        config_path: PathBuf,
        exchange: &'a dyn MemoryExchange,
        work_tree: &'a dyn SyncWorkTree,
    ) -> Self {
        Self {
            git,
            global_dir,
            config_path,
            exchange,
            work_tree,
        }
    }

    fn sync_dir(&self) -> PathBuf {
        self.global_dir.join("sync")
    }

    /// Idempotent: re-running repoints the remote rather than failing.
    pub async fn init(&self, remote_url: &str) -> AgentResult<String> {
        self.validate_remote_url(remote_url).await?;
        let dir = self.sync_dir();
        self.work_tree.ensure_dir(&dir).await?;
        if !self.work_tree.exists(&dir.join(".git")).await? {
            self.git_ok(&["init"], &dir).await?;
            // Best-effort: renaming an unborn branch no-ops on some git versions, harmlessly.
            let _ = self.git.run(&["branch", "-m", SYNC_BRANCH], &dir).await;
        }
        self.work_tree
            .write(&dir.join(".gitignore"), GITIGNORE)
            .await?;
        // A missing `origin` on first init makes this fail, which is exactly the state we want anyway.
        let _ = self.git.run(&["remote", "remove", "origin"], &dir).await;
        self.git_ok(&["remote", "add", "origin", remote_url], &dir)
            .await?;
        Ok(format!(
            "sync initialized at {} → {remote_url}",
            dir.display()
        ))
    }

    /// A no-op commit (nothing changed) is not an error.
    pub async fn push(&self) -> AgentResult<String> {
        let dir = self.require_initialized().await?;
        let count = self.export_profile(&dir).await?;
        self.git_ok(&["add", "-A"], &dir).await?;
        // An empty commit exits non-zero, so "nothing to commit" has to be read as success.
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

    /// A risky config change is refused unless `force`.
    pub async fn pull(&self, force: bool) -> AgentResult<String> {
        let dir = self.require_initialized().await?;
        // Hard-resetting is safe because the tree holds only export artifacts — the live memory and config
        // are outside it. It also handles a fresh clone whose local branch is unborn, where `pull
        // --ff-only` fails.
        self.git_ok(&["fetch", "origin", SYNC_BRANCH], &dir).await?;
        self.git_ok(&["reset", "--hard", "FETCH_HEAD"], &dir)
            .await?;

        let report = self.exchange.import(&dir.join(MEMORY_FILE)).await?;

        let incoming_config = dir.join(CONFIG_FILE);
        let config_note = match self.work_tree.read_to_string(&incoming_config).await? {
            Some(incoming) => {
                // Valid TOML but invalid schema is refused even under `force`: writing it would brick the
                // next boot, which `force` cannot consent to.
                if let Err(error) = config::validate_config_str(&incoming) {
                    format!("config NOT applied ({error})")
                } else {
                    // An absent current config is an empty baseline; an unreadable one is no baseline at
                    // all, so the change cannot be proven safe.
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

    pub async fn status(&self) -> AgentResult<String> {
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
        // `--branch` always prints a `## <branch>` header, so empty output is never the clean signal.
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

    async fn export_profile(&self, dir: &Path) -> AgentResult<usize> {
        self.work_tree
            .write(&dir.join(".gitignore"), GITIGNORE)
            .await?;
        if self.work_tree.exists(&self.config_path).await? {
            self.work_tree
                .copy(&self.config_path, &dir.join(CONFIG_FILE))
                .await?;
        }
        self.exchange.export(&dir.join(MEMORY_FILE)).await
    }

    async fn require_initialized(&self) -> AgentResult<PathBuf> {
        let dir = self.sync_dir();
        if !self.work_tree.exists(&dir.join(".git")).await? {
            return Err(AgentError::Sync(
                "sync not initialized — run `kiri sync init <repo-url>` first".to_string(),
            ));
        }
        Ok(dir)
    }

    async fn git_ok(&self, args: &[&str], cwd: &Path) -> AgentResult<()> {
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

    /// `git` reads some "URLs" as code-execution transports (`ext::sh -c …`, any `<helper>::…`) or as
    /// options (`-oProxyCommand=…`), and `git remote add` takes no `--` end-of-options marker. A positive
    /// allowlist is the only defense.
    async fn validate_remote_url(&self, url: &str) -> AgentResult<()> {
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

    async fn is_allowed_remote_url(&self, url: &str) -> AgentResult<bool> {
        const SCHEMES: [&str; 3] = ["https://", "http://", "ssh://"];
        if let Some(rest) = SCHEMES.iter().find_map(|scheme| url.strip_prefix(scheme)) {
            return Ok(scheme_authority_is_safe(rest));
        }
        if is_scp_like(url) {
            return Ok(true);
        }
        // Checked before the local-path arm, so a real file named `evil::payload` cannot slip through as
        // a remote helper.
        if url.contains("::") {
            return Ok(false);
        }
        let path = Path::new(url);
        // On Windows `Path::is_absolute` demands a drive prefix, so a Unix-style path would read as
        // relative without the explicit leading-`/` test.
        Ok(url.starts_with('/') || path.is_absolute() || self.work_tree.exists(path).await?)
    }
}

/// Requiring an alphanumeric first char keeps a leading `-` from masquerading as a `user` segment.
fn is_scp_like(url: &str) -> bool {
    let Some((authority, _path)) = url.split_once(':') else {
        return false;
    };
    let Some((user, host)) = authority.split_once('@') else {
        return false;
    };
    is_host_segment(user) && is_host_segment(host)
}

/// Narrower than [`is_host_segment`] on purpose: it rejects only a leading `-` (the option-injection
/// vector), leaving IPv6 literals and userinfo-with-password — both legitimate remotes — alone.
fn scheme_authority_is_safe(rest: &str) -> bool {
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return false;
    }
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    if userinfo.is_some_and(|user| user.starts_with('-')) {
        return false;
    }
    let host = hostport.split(':').next().unwrap_or("");
    !host.is_empty() && !host.starts_with('-')
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

fn first_line(stderr: &str, stdout: &str) -> String {
    let pick = if stderr.is_empty() { stdout } else { stderr };
    pick.lines().next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::application::shared_memory::SharedMemory;
    use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
    use crate::modules::sync::application::git::GitOutput;
    use crate::modules::sync::infrastructure::fs_work_tree::FsSyncWorkTree;
    use crate::modules::sync::infrastructure::memory_ndjson::NdjsonMemoryExchange;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn sync_service_has_no_inline_fs() {
        let source = include_str!("sync_service.rs");
        // The test module legitimately uses std::fs, so only the slice before it is scanned. The needles
        // are built by concatenation so this guard's own literals do not self-match.
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

    /// Always succeeds. On `reset` it materializes the fixture files, standing in for the remote.
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

    #[async_trait::async_trait]
    impl Git for FakeGit {
        async fn run(&self, args: &[&str], cwd: &Path) -> AgentResult<GitOutput> {
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
        let mem = SqliteSharedMemory::new(global.join("memory").join("shared.db")).unwrap();
        mem.init().await.unwrap();
        let exchange = NdjsonMemoryExchange::new(&mem);
        let git = FakeGit::new();
        let work_tree = FsSyncWorkTree;
        let service = SyncService::new(&git, global.clone(), config, &exchange, &work_tree);

        service.init("git@example:me/profile.git").await.unwrap();
        service.push().await.unwrap();

        let sync_dir = global.join("sync");
        assert!(sync_dir.join("memory.ndjson").exists());
        assert!(sync_dir.join("config.toml").exists());
        assert!(sync_dir.join(".gitignore").exists());
        assert!(!sync_dir.join("credentials.json").exists());
        let calls = git.calls.lock().unwrap();
        assert!(calls.iter().any(|c| c.starts_with("push")));
    }

    #[tokio::test]
    async fn push_before_init_errors() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().to_path_buf();
        let mem = SqliteSharedMemory::in_memory().unwrap();
        let exchange = NdjsonMemoryExchange::new(&mem);
        let git = FakeGit::new();
        let work_tree = FsSyncWorkTree;
        let service = SyncService::new(
            &git,
            global.clone(),
            global.join("config.toml"),
            &exchange,
            &work_tree,
        );
        assert!(service.push().await.is_err());
    }

    fn url_service<'a>(
        git: &'a FakeGit,
        exchange: &'a dyn MemoryExchange,
        work_tree: &'a FsSyncWorkTree,
    ) -> SyncService<'a> {
        SyncService::new(
            git,
            PathBuf::from("/kiri-url-test"),
            PathBuf::from("/kiri-url-test/config.toml"),
            exchange,
            work_tree,
        )
    }

    #[tokio::test]
    async fn validate_remote_url_rejects_ext_and_dash() {
        let git = FakeGit::new();
        let mem = SqliteSharedMemory::in_memory().unwrap();
        let exchange = NdjsonMemoryExchange::new(&mem);
        let work_tree = FsSyncWorkTree;
        let s = url_service(&git, &exchange, &work_tree);
        assert!(
            s.validate_remote_url("ext::sh -c 'rm -rf ~'")
                .await
                .is_err()
        );
        assert!(s.validate_remote_url("fab::evil").await.is_err());
        assert!(
            s.validate_remote_url("EXT::sh -c 'rm -rf ~'")
                .await
                .is_err()
        );
        assert!(s.validate_remote_url("fd::evil").await.is_err());
        assert!(s.validate_remote_url("-oProxyCommand=evil").await.is_err());
        assert!(s.validate_remote_url("file://-evil").await.is_err());
        assert!(s.validate_remote_url("https://-evil.com/x").await.is_err());
        assert!(s.validate_remote_url("http://-evil/x").await.is_err());
        assert!(
            s.validate_remote_url("ssh://-oProxyCommand=evil@h/x")
                .await
                .is_err()
        );
        assert!(s.validate_remote_url("https:///no-host").await.is_err());
        assert!(s.validate_remote_url("   ").await.is_err());
        assert!(s.validate_remote_url("/tmp/evil::payload").await.is_err());
    }

    #[tokio::test]
    async fn validate_remote_url_accepts_https_ssh_and_local() {
        let git = FakeGit::new();
        let mem = SqliteSharedMemory::in_memory().unwrap();
        let exchange = NdjsonMemoryExchange::new(&mem);
        let work_tree = FsSyncWorkTree;
        let s = url_service(&git, &exchange, &work_tree);
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

    fn pull_service<'a>(
        git: &'a FakeGit,
        exchange: &'a dyn MemoryExchange,
        work_tree: &'a FsSyncWorkTree,
        global: &Path,
        current_config: &str,
    ) -> (SyncService<'a>, PathBuf) {
        let config = global.join("config.toml");
        std::fs::write(&config, current_config).unwrap();
        let service = SyncService::new(
            git,
            global.to_path_buf(),
            config.clone(),
            exchange,
            work_tree,
        );
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
        let exchange = NdjsonMemoryExchange::new(&mem);
        let work_tree = FsSyncWorkTree;
        let (service, config) = pull_service(&git, &exchange, &work_tree, dir.path(), &current);
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
        let exchange = NdjsonMemoryExchange::new(&mem);
        let work_tree = FsSyncWorkTree;
        let (service, config) = pull_service(&git, &exchange, &work_tree, dir.path(), &current);
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
        let exchange = NdjsonMemoryExchange::new(&mem);
        let work_tree = FsSyncWorkTree;
        let (service, config) = pull_service(&git, &exchange, &work_tree, dir.path(), &config_text);
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
        let exchange = NdjsonMemoryExchange::new(&mem);
        let work_tree = FsSyncWorkTree;
        let (service, _config) = pull_service(
            &git,
            &exchange,
            &work_tree,
            dir.path(),
            &provider_toml("nvidia", "https://x/v1"),
        );
        service.init("git@example:me/p.git").await.unwrap();

        let summary = service.pull(false).await.unwrap();
        assert!(summary.contains("NOT applied"), "{summary}");
    }
}
