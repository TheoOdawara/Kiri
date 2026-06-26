use std::path::{Path, PathBuf};

use tokio::fs;

use crate::modules::memory::domain::project_memory::SharedMemory;
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::modules::sync::application::git::Git;
use crate::modules::sync::infrastructure::memory_ndjson;
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
    /// The shared memory database, exported to / imported from NDJSON.
    shared_db: PathBuf,
}

impl<'a> SyncService<'a> {
    pub fn new(
        git: &'a dyn Git,
        global_dir: PathBuf,
        config_path: PathBuf,
        shared_db: PathBuf,
    ) -> Self {
        Self {
            git,
            global_dir,
            config_path,
            shared_db,
        }
    }

    fn sync_dir(&self) -> PathBuf {
        self.global_dir.join("sync")
    }

    /// Initialize the sync work-tree and point it at `remote_url`. Idempotent: re-running updates the
    /// remote URL rather than failing.
    pub async fn init(&self, remote_url: &str) -> Result<String> {
        let dir = self.sync_dir();
        fs::create_dir_all(&dir).await?;
        if !dir.join(".git").exists() {
            self.git_ok(&["init"], &dir).await?;
            // Pin the branch name so push/pull agree regardless of the host's init.defaultBranch.
            // Best-effort: renaming an unborn branch can no-op on some git versions.
            let _ = self.git.run(&["branch", "-m", SYNC_BRANCH], &dir).await;
        }
        fs::write(dir.join(".gitignore"), GITIGNORE).await?;
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
            return Err(AgentError::Provider(format!(
                "git commit failed: {}",
                first_line(&commit.stderr, &commit.stdout)
            )));
        }
        let push = self
            .git
            .run(&["push", "-u", "origin", SYNC_BRANCH], &dir)
            .await?;
        if !push.success {
            return Err(AgentError::Provider(format!(
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
        let memory = SqliteSharedMemory::new(self.shared_db.clone())?;
        memory.init().await?;
        let report = memory_ndjson::import(&memory, &dir.join(MEMORY_FILE)).await?;

        // Apply config under the trust check.
        let incoming_config = dir.join(CONFIG_FILE);
        let config_note = if incoming_config.exists() {
            let incoming = fs::read_to_string(&incoming_config).await?;
            let current = fs::read_to_string(&self.config_path)
                .await
                .unwrap_or_default();
            let risks = risky_config_changes(&current, &incoming);
            if risks.is_empty() || force {
                fs::write(&self.config_path, incoming).await?;
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
        } else {
            "no config in sync".to_string()
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
        Ok(if output.stdout.is_empty() {
            "sync clean".to_string()
        } else {
            output.stdout
        })
    }

    /// Write the non-secret profile into the work-tree, returning the exported memory-entry count.
    async fn export_profile(&self, dir: &Path) -> Result<usize> {
        fs::write(dir.join(".gitignore"), GITIGNORE).await?;
        if self.config_path.exists() {
            fs::copy(&self.config_path, dir.join(CONFIG_FILE)).await?;
        }
        let memory = SqliteSharedMemory::new(self.shared_db.clone())?;
        memory.init().await?;
        memory_ndjson::export(&memory, &dir.join(MEMORY_FILE)).await
    }

    async fn require_initialized(&self) -> Result<PathBuf> {
        let dir = self.sync_dir();
        if !dir.join(".git").exists() {
            return Err(AgentError::Provider(
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
            Err(AgentError::Provider(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or(""),
                first_line(&output.stderr, &output.stdout)
            )))
        }
    }
}

/// The first non-empty line of stderr, falling back to stdout, for a compact error message.
fn first_line(stderr: &str, stdout: &str) -> String {
    let pick = if stderr.is_empty() { stdout } else { stderr };
    pick.lines().next().unwrap_or("").to_string()
}

/// Identify risky differences in an incoming config that, applied as the trusted global layer, could
/// redirect a credential or weaken the sandbox: a provider's `base_url` changing, or the sandbox mode
/// being set to `off`. Returns a human-readable list (empty = safe to apply).
fn risky_config_changes(current: &str, incoming: &str) -> Vec<String> {
    let current: toml::Value = current
        .parse()
        .unwrap_or(toml::Value::Table(Default::default()));
    let incoming: toml::Value = match incoming.parse() {
        Ok(value) => value,
        // An unparseable incoming config is itself a reason not to apply it blindly.
        Err(error) => return vec![format!("incoming config is not valid TOML: {error}")],
    };

    let mut risks = Vec::new();

    // Provider base_url changes (credential-redirect risk).
    if let (Some(cur), Some(inc)) = (
        current.get("providers").and_then(|v| v.as_table()),
        incoming.get("providers").and_then(|v| v.as_table()),
    ) {
        for (id, inc_profile) in inc {
            let inc_url = inc_profile.get("base_url").and_then(|v| v.as_str());
            let cur_url = cur
                .get(id)
                .and_then(|p| p.get("base_url"))
                .and_then(|v| v.as_str());
            if let (Some(cur_url), Some(inc_url)) = (cur_url, inc_url)
                && cur_url != inc_url
            {
                risks.push(format!("provider '{id}' base_url changes to {inc_url}"));
            }
        }
    }

    // Sandbox weakened to off.
    let inc_mode = incoming
        .get("sandbox")
        .and_then(|v| v.get("mode"))
        .and_then(|v| v.as_str());
    if inc_mode == Some("off") {
        risks.push("sandbox mode set to 'off'".to_string());
    }

    risks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::sync::application::git::GitOutput;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// A `Git` double that records the commands it was asked to run and always succeeds. It also creates
    /// the `.git` marker on `init`, so `require_initialized` passes in later calls.
    struct FakeGit {
        calls: Mutex<Vec<String>>,
    }

    impl FakeGit {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
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
            Ok(GitOutput {
                stdout: String::new(),
                stderr: String::new(),
                success: true,
            })
        }
    }

    #[tokio::test]
    async fn init_then_push_writes_profile_and_runs_git() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().to_path_buf();
        let config = global.join("config.toml");
        std::fs::write(&config, "[providers.nvidia]\nbase_url = \"https://x/v1\"\n").unwrap();
        // A shared.db so export has something to read.
        let shared = global.join("memory").join("shared.db");
        let git = FakeGit::new();
        let service = SyncService::new(&git, global.clone(), config, shared);

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
        let git = FakeGit::new();
        let service = SyncService::new(
            &git,
            global.clone(),
            global.join("config.toml"),
            global.join("memory").join("shared.db"),
        );
        assert!(service.push().await.is_err());
    }

    #[test]
    fn detects_base_url_change() {
        let current = r#"
[providers.nvidia]
base_url = "https://integrate.api.nvidia.com/v1"
"#;
        let incoming = r#"
[providers.nvidia]
base_url = "https://evil.example/v1"
"#;
        let risks = risky_config_changes(current, incoming);
        assert_eq!(risks.len(), 1);
        assert!(risks[0].contains("nvidia"));
    }

    #[test]
    fn detects_sandbox_off() {
        let risks = risky_config_changes("", "[sandbox]\nmode = \"off\"\n");
        assert!(risks.iter().any(|r| r.contains("sandbox")));
    }

    #[test]
    fn identical_config_is_safe() {
        let config = "[providers.nvidia]\nbase_url = \"https://x/v1\"\n";
        assert!(risky_config_changes(config, config).is_empty());
    }
}
