use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::fs;

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::sync::application::git::Git;
use crate::modules::sync::infrastructure::memory_ndjson;
use crate::shared::infra::config;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::AuthMethod;
use crate::shared::kernel::sandbox::{NetworkStance, SandboxMode};

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
}

impl<'a> SyncService<'a> {
    pub fn new(
        git: &'a dyn Git,
        global_dir: PathBuf,
        config_path: PathBuf,
        memory: &'a dyn SharedMemory,
    ) -> Self {
        Self {
            git,
            global_dir,
            config_path,
            memory,
        }
    }

    fn sync_dir(&self) -> PathBuf {
        self.global_dir.join("sync")
    }

    /// Initialize the sync work-tree and point it at `remote_url`. Idempotent: re-running updates the
    /// remote URL rather than failing. The URL is validated before it reaches `git remote add` (which
    /// has no `--` end-of-options marker), so a hostile transport cannot smuggle command execution.
    pub async fn init(&self, remote_url: &str) -> Result<String> {
        validate_remote_url(remote_url)?;
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

        // Apply config under the trust check.
        let incoming_config = dir.join(CONFIG_FILE);
        let config_note = if incoming_config.exists() {
            let incoming = fs::read_to_string(&incoming_config).await?;
            // Refuse a config that is valid TOML but invalid against the real schema, regardless of
            // `force` — writing it would brick the next boot when it fails to deserialize.
            if let Err(error) = config::validate_config_str(&incoming) {
                format!("config NOT applied ({error})")
            } else {
                // Establish the trusted baseline. A genuinely absent current config is an empty
                // baseline (first pull); a present-but-unreadable one cannot be trusted, so we cannot
                // prove the change is safe and require `--force`.
                let current = match fs::read_to_string(&self.config_path).await {
                    Ok(text) => Some(text),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(String::new()),
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
                    write_atomic(&self.config_path, &incoming).await?;
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
        fs::write(dir.join(".gitignore"), GITIGNORE).await?;
        if self.config_path.exists() {
            fs::copy(&self.config_path, dir.join(CONFIG_FILE)).await?;
        }
        memory_ndjson::export(self.memory, &dir.join(MEMORY_FILE)).await
    }

    async fn require_initialized(&self) -> Result<PathBuf> {
        let dir = self.sync_dir();
        if !dir.join(".git").exists() {
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
}

/// Validate a `kiri sync init` remote URL before it reaches `git remote add`. `git` treats some "URLs"
/// as code-execution transports (`ext::sh -c …`, `fab::…`) or as options (a leading `-`, e.g.
/// `-oProxyCommand=…`, and a `file://-…` dash-path), any of which turns an attacker-controlled URL into a
/// command-injection vector. `git remote add` takes no `--` end-of-options marker, so this validation is
/// the defense: reject those dangerous shapes; accept the ordinary transports (https/http/ssh, scp-like
/// `git@host:path`) and an ordinary local path.
fn validate_remote_url(url: &str) -> Result<()> {
    let url = url.trim();
    if url.is_empty() {
        return Err(AgentError::Sync("sync remote URL is empty".to_string()));
    }
    if url.starts_with('-') {
        return Err(AgentError::Sync(format!(
            "refusing remote URL parsed by git as an option (leading '-'): {url}"
        )));
    }
    if url.starts_with("file://-") {
        return Err(AgentError::Sync(format!(
            "refusing file:// URL with a leading-dash path: {url}"
        )));
    }
    for transport in ["ext::", "fab::"] {
        if url.starts_with(transport) {
            return Err(AgentError::Sync(format!(
                "refusing remote transport '{transport}' (arbitrary command execution): {url}"
            )));
        }
    }
    Ok(())
}

/// The first non-empty line of stderr, falling back to stdout, for a compact error message.
fn first_line(stderr: &str, stdout: &str) -> String {
    let pick = if stderr.is_empty() { stdout } else { stderr };
    pick.lines().next().unwrap_or("").to_string()
}

/// Write `contents` to `path` atomically: write a sibling temp file then rename over the target, so a
/// crash mid-write can never leave a truncated/corrupt trusted config.
async fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml");
    let tmp = path.with_file_name(format!(".{name}.kiri-tmp"));
    fs::write(&tmp, contents).await?;
    fs::rename(&tmp, path).await?;
    Ok(())
}

/// The security-relevant fields of a config, parsed against the real shape (extra fields ignored).
/// Deliberately a small typed view rather than `toml::Value` poking, so the trust gate reasons over the
/// same schema the loader uses and cannot be fooled by an unexpected layout.
#[derive(Deserialize, Default)]
struct TrustView {
    #[serde(default)]
    active_provider: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, TrustProvider>,
    #[serde(default)]
    sandbox: TrustSandbox,
    #[serde(default)]
    embeddings: TrustEmbeddings,
}

#[derive(Deserialize, Default)]
struct TrustProvider {
    #[serde(default)]
    base_url: Option<String>,
    /// Typed against the kernel [`AuthMethod`] (forward-compat `Deserialize`) so the gate reasons over
    /// the same enum the loader uses, not a hand-typed `"none"` literal. Absent = the historical default.
    #[serde(default)]
    auth: Option<AuthMethod>,
}

#[derive(Deserialize, Default)]
struct TrustSandbox {
    /// Typed against the kernel sandbox primitives; an unknown value maps to the safe variant, so a
    /// forward-version config is never read as a downgrade. Absent = the resolver's baseline.
    #[serde(default)]
    mode: Option<SandboxMode>,
    #[serde(default)]
    network: Option<NetworkStance>,
}

#[derive(Deserialize, Default)]
struct TrustEmbeddings {
    #[serde(default)]
    provider: Option<String>,
}

/// Identify risky differences in an incoming config that, applied as the trusted global layer, could
/// redirect a credential or weaken the sandbox. Flags: a newly added provider; an existing provider's
/// `base_url` added or changed; an existing provider's auth disabled; the active provider switching to a
/// different endpoint; the embeddings provider changing; the sandbox confinement *weakened* by rank
/// (`require → os`, `require → off`, `os → off`); the sandbox network widened to allow. Reasons over the
/// typed kernel [`AuthMethod`]/[`SandboxMode`]/[`NetworkStance`] (no magic strings). Returns a
/// human-readable list (empty = safe to apply). Schema validity is checked separately by the caller.
fn risky_config_changes(current: &str, incoming: &str) -> Vec<String> {
    let incoming: TrustView = match toml::from_str(incoming) {
        Ok(value) => value,
        Err(error) => return vec![format!("incoming config is not valid TOML: {error}")],
    };
    // A current config we cannot parse is not a baseline we can compare against — we cannot prove the
    // change is non-risky, so treat it as requiring an explicit `--force`.
    let current: TrustView = match toml::from_str(current) {
        Ok(value) => value,
        Err(_) => {
            return vec![
                "current config is unreadable; cannot verify the change is safe".to_string(),
            ];
        }
    };

    let mut risks = Vec::new();

    // A new provider, or a base_url added/changed on an existing one, can redirect where a credential
    // (stored or env-imported) is sent — the core credential-exfiltration vector. Turning off an existing
    // provider's authentication (api-key/oauth -> none) silently disables its credential, and for a vendor
    // endpoint the next boot then fails to build it (a DoS-via-sync); both require an explicit `--force`.
    for (id, inc) in &incoming.providers {
        match current.providers.get(id) {
            None => risks.push(format!("new provider '{id}' added")),
            Some(cur) => {
                if cur.base_url != inc.base_url {
                    risks.push(format!("provider '{id}' base_url changes"));
                }
                // Anything not explicitly `None` (api-key/oauth, or absent = the historical default,
                // since `None != Some(AuthMethod::None)`) is treated as keyed, so dropping authentication
                // is always flagged.
                if cur.auth != Some(AuthMethod::None) && inc.auth == Some(AuthMethod::None) {
                    risks.push(format!("provider '{id}' auth disabled"));
                }
            }
        }
    }

    // The active provider switching to one with a different base_url redirects the active credential.
    let active_url = |view: &TrustView| -> Option<String> {
        view.active_provider
            .as_ref()
            .and_then(|id| view.providers.get(id))
            .and_then(|p| p.base_url.clone())
    };
    if incoming.active_provider != current.active_provider
        && active_url(&incoming) != active_url(&current)
    {
        risks.push("active_provider changes to a different endpoint".to_string());
    }

    // Redirecting the embeddings provider sends the embedded text (and that provider's key) elsewhere.
    if incoming.embeddings.provider != current.embeddings.provider {
        risks.push("embeddings provider changes".to_string());
    }

    // Sandbox confinement must not weaken. Rank the modes (`Require > Os > Off`) so any strictly-lower
    // incoming rank is flagged — not only `→ off`. An absent mode is the resolver's `Os` baseline, so
    // `absent → os` (and `os → require`) does not flag. Debug-format the modes so the message carries no
    // bare `"off"`/`"require"` literal the gate could be mistaken for comparing against.
    let current_mode = current.sandbox.mode.unwrap_or(SandboxMode::Os);
    let incoming_mode = incoming.sandbox.mode.unwrap_or(SandboxMode::Os);
    if incoming_mode.rank() < current_mode.rank() {
        risks.push(format!(
            "sandbox confinement weakened ({current_mode:?} -> {incoming_mode:?})"
        ));
    }

    // Base network stance must not widen from deny to allow (an absent stance is the `Deny` baseline).
    let current_net = current.sandbox.network.unwrap_or(NetworkStance::Deny);
    let incoming_net = incoming.sandbox.network.unwrap_or(NetworkStance::Deny);
    if incoming_net == NetworkStance::Allow && current_net != NetworkStance::Allow {
        risks.push(format!(
            "sandbox network widened ({current_net:?} -> {incoming_net:?})"
        ));
    }

    risks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
    use crate::modules::sync::application::git::GitOutput;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tempfile::TempDir;

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
        let service = SyncService::new(&git, global.clone(), config, &mem);

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
        let service = SyncService::new(&git, global.clone(), global.join("config.toml"), &mem);
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
    fn detects_sandbox_require_to_os_downgrade() {
        // The audited hole: `require → os` is a genuine weakening on a platform with no OS sandbox, yet
        // the old gate only flagged `→ off`. Ranking catches it.
        let risks = risky_config_changes(
            "[sandbox]\nmode = \"require\"\n",
            "[sandbox]\nmode = \"os\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("sandbox")), "{risks:?}");
    }

    #[test]
    fn detects_sandbox_require_to_off_downgrade() {
        let risks = risky_config_changes(
            "[sandbox]\nmode = \"require\"\n",
            "[sandbox]\nmode = \"off\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("sandbox")), "{risks:?}");
    }

    #[test]
    fn detects_sandbox_os_to_off_downgrade() {
        let risks =
            risky_config_changes("[sandbox]\nmode = \"os\"\n", "[sandbox]\nmode = \"off\"\n");
        assert!(risks.iter().any(|r| r.contains("sandbox")), "{risks:?}");
    }

    #[test]
    fn sandbox_os_to_require_is_safe() {
        // Strengthening confinement (lower → higher rank) is never risky.
        let risks = risky_config_changes(
            "[sandbox]\nmode = \"os\"\n",
            "[sandbox]\nmode = \"require\"\n",
        );
        assert!(
            !risks.iter().any(|r| r.contains("sandbox")),
            "strengthening must not flag: {risks:?}"
        );
    }

    #[test]
    fn sandbox_absent_to_os_is_safe() {
        // An absent mode is the `Os` baseline, so `absent → os` is a no-op, not a downgrade.
        let risks = risky_config_changes("", "[sandbox]\nmode = \"os\"\n");
        assert!(
            !risks.iter().any(|r| r.contains("sandbox")),
            "absent baseline is os: {risks:?}"
        );
    }

    #[test]
    fn auth_gate_uses_typed_authmethod() {
        // The auth-disabled check reasons over the typed `Some(AuthMethod::None)`, not a `"none"` string:
        // an `api-key → none` change on the same endpoint is flagged.
        let current = provider_toml("nvidia", "https://x/v1"); // auth = "api-key"
        let incoming = "[providers.nvidia]\nkind = \"open-ai-compatible\"\n\
                        base_url = \"https://x/v1\"\nmodel = \"m\"\nauth = \"none\"\n";
        let risks = risky_config_changes(&current, incoming);
        assert!(
            risks.iter().any(|r| r.contains("auth disabled")),
            "{risks:?}"
        );
    }

    #[test]
    fn identical_config_is_safe() {
        let config = "[providers.nvidia]\nbase_url = \"https://x/v1\"\n";
        assert!(risky_config_changes(config, config).is_empty());
    }

    #[test]
    fn detects_a_new_provider() {
        let current = "[providers.nvidia]\nbase_url = \"https://x/v1\"\n";
        let incoming = "[providers.nvidia]\nbase_url = \"https://x/v1\"\n\
                        [providers.evil]\nbase_url = \"https://attacker/v1\"\n";
        let risks = risky_config_changes(current, incoming);
        assert!(risks.iter().any(|r| r.contains("evil")), "{risks:?}");
    }

    #[test]
    fn detects_auth_downgrade_to_none() {
        // Turning off authentication on an existing provider (same base_url) must require --force, so a
        // synced config cannot silently disable a credential — and, for a vendor endpoint, brick the boot.
        let current = provider_toml("nvidia", "https://x/v1"); // auth = "api-key"
        let incoming = "[providers.nvidia]\nkind = \"open-ai-compatible\"\n\
                        base_url = \"https://x/v1\"\nmodel = \"m\"\nauth = \"none\"\n";
        let risks = risky_config_changes(&current, incoming);
        assert!(
            risks.iter().any(|r| r.contains("auth disabled")),
            "{risks:?}"
        );
    }

    #[test]
    fn already_keyless_provider_unchanged_is_safe() {
        // A provider that was already keyless (none -> none) is not flagged as a downgrade.
        let config = "[providers.lmstudio]\nkind = \"open-ai-compatible\"\n\
                      base_url = \"http://localhost:1234/v1\"\nmodel = \"m\"\nauth = \"none\"\n";
        assert!(risky_config_changes(config, config).is_empty());
    }

    #[test]
    fn detects_active_provider_redirect() {
        let current = "active_provider = \"a\"\n[providers.a]\nbase_url = \"https://a/v1\"\n\
                       [providers.b]\nbase_url = \"https://b/v1\"\n";
        let incoming = "active_provider = \"b\"\n[providers.a]\nbase_url = \"https://a/v1\"\n\
                        [providers.b]\nbase_url = \"https://b/v1\"\n";
        let risks = risky_config_changes(current, incoming);
        assert!(
            risks.iter().any(|r| r.contains("active_provider")),
            "{risks:?}"
        );
    }

    #[test]
    fn detects_sandbox_network_widened() {
        let risks = risky_config_changes(
            "[sandbox]\nnetwork = \"deny\"\n",
            "[sandbox]\nnetwork = \"allow\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("network")), "{risks:?}");
    }

    #[test]
    fn detects_embeddings_provider_change() {
        let risks = risky_config_changes(
            "[embeddings]\nprovider = \"nvidia\"\n",
            "[embeddings]\nprovider = \"evil\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("embeddings")), "{risks:?}");
    }

    #[test]
    fn unreadable_current_config_is_treated_as_risky() {
        let risks = risky_config_changes("this is = = not toml", "[sandbox]\nmode = \"os\"\n");
        assert!(!risks.is_empty());
    }

    #[test]
    fn validate_remote_url_rejects_ext_and_dash() {
        assert!(validate_remote_url("ext::sh -c 'rm -rf ~'").is_err());
        assert!(validate_remote_url("fab::evil").is_err());
        assert!(validate_remote_url("-oProxyCommand=evil").is_err());
        assert!(validate_remote_url("file://-evil").is_err());
        assert!(validate_remote_url("   ").is_err());
    }

    #[test]
    fn validate_remote_url_accepts_https_ssh_and_local() {
        assert!(validate_remote_url("https://github.com/me/profile.git").is_ok());
        assert!(validate_remote_url("http://example.test/p.git").is_ok());
        assert!(validate_remote_url("ssh://git@host/me/profile.git").is_ok());
        assert!(validate_remote_url("git@github.com:me/profile.git").is_ok());
        assert!(validate_remote_url("/home/me/profiles/p.git").is_ok());
    }

    // End-to-end pull: FakeGit's `reset` materializes the remote's config + memory into the work-tree,
    // exercising the apply/refuse decision and the --force override on the security-weighted path.
    fn pull_service<'a>(
        git: &'a FakeGit,
        memory: &'a dyn SharedMemory,
        global: &Path,
        current_config: &str,
    ) -> (SyncService<'a>, PathBuf) {
        let config = global.join("config.toml");
        std::fs::write(&config, current_config).unwrap();
        let service = SyncService::new(git, global.to_path_buf(), config.clone(), memory);
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
        let (service, config) = pull_service(&git, &mem, dir.path(), &current);
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
        let (service, config) = pull_service(&git, &mem, dir.path(), &current);
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
        let (service, config) = pull_service(&git, &mem, dir.path(), &config_text);
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
        let (service, _config) = pull_service(
            &git,
            &mem,
            dir.path(),
            &provider_toml("nvidia", "https://x/v1"),
        );
        service.init("git@example:me/p.git").await.unwrap();

        let summary = service.pull(false).await.unwrap();
        assert!(summary.contains("NOT applied"), "{summary}");
    }
}
