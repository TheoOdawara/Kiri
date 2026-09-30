use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use regex::Regex;

use crate::shared::kernel::provider::{Effort, ProviderProfile};
use crate::shared::kernel::sandbox::NetworkPolicy;

use super::defaults::{
    DEFAULT_PLAN_ALLOW, DEFAULT_RW_DIRS, HTTP_CONNECT_TIMEOUT, HTTP_READ_TIMEOUT,
    MAX_TOOL_CALLS_PER_CHECKPOINT, TOOL_CHECKPOINT,
};
use super::raw::{
    read_config_file, read_project_config_lenient, resolve_layers, resolve_providers,
};
use super::resolve::{
    compile_patterns, expand_home, load_extra_paths, resolve_bool, resolve_sandbox_mode,
    resolve_sandbox_network, resolve_timeout,
};
use super::writers::{default_provider, ensure_private_dir, write_starter_config};

/// Private: only `Settings::resolve` derives it. Every consumer reads the resolved `Settings::global_dir`,
/// the single harness-home source (ADR 0015).
fn kiri_global_dir() -> PathBuf {
    expand_home("~/.kiri")
}

/// Seeds process env from `~/.kiri/.env` before config resolution. Read ONLY from the trusted global dir,
/// never the cwd: a hostile project repo must not inject env and thereby redirect a credential or weaken
/// the sandbox (ADR 0020). `dotenvy` never overrides an already-exported var.
pub fn load_global_env() {
    let env_path = kiri_global_dir().join(".env");
    // Deliberately ignored: `.env` is an optional convenience. A missing file, or a malformed line that
    // fails to parse, must not abort boot — the affected key simply stays unset and onboarding handles it.
    let _ = dotenvy::from_path(&env_path);
}

/// The resolved configuration the composition root needs to wire the harness. The matching secret is
/// fetched from the credential store at wire time, never stored here.
pub struct Settings {
    pub path: PathBuf,
    pub seed: Option<String>,
    pub checkpoint_budget: Duration,
    pub max_tool_calls: usize,
    pub plan_allow: Arc<[Regex]>,
    /// Whether OS-level command confinement is active (`KIRI_SANDBOX` ≠ `off`, facility available).
    pub sandbox_enabled: bool,
    /// `KIRI_SANDBOX=require`: refuse `run_command` when no OS sandbox is available.
    pub require_confinement: bool,
    /// Base network stance for `run_command` — deny by default, `KIRI_SANDBOX_NETWORK=allow` widens it
    /// session-wide; no per-command widening (ADR 0022).
    pub sandbox_network: NetworkPolicy,
    /// Extra paths a confined command may read / write beyond the workspace (toolchain dirs, config).
    pub extra_ro: Arc<[PathBuf]>,
    pub extra_rw: Arc<[PathBuf]>,
    /// `read_timeout` caps idle time between received bytes, so it is streaming-safe. Both bound a hung
    /// provider so a turn fails fast instead of hanging silently.
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    /// Disable for a model that rejects or stalls on streamed reasoning.
    pub thinking: bool,
    pub memory_enabled: bool,
    /// What `consult_docs` searches. Defaults to `<path>/docs`.
    pub docs_path: PathBuf,
    /// Defaults to `~/.kiri/memory/shared.db`.
    pub shared_memory_db: PathBuf,
    /// Defaults to `~/.kiri/sessions.db`. Gated by `memory_enabled`.
    pub sessions_db: PathBuf,
    pub credentials_file: PathBuf,
    /// The harness home. Every consumer reads this instead of re-deriving `config_path.parent()`.
    pub global_dir: PathBuf,
    /// The trusted layer, and the only one the runtime writes live `/models`/`/effort` changes back to.
    pub config_path: PathBuf,
    pub providers: Vec<ProviderProfile>,
    /// Must name one of `providers`.
    pub active_provider: String,
    pub effort: Effort,
    /// `None` keeps recall keyword-only. Trusted (global) layer only.
    pub embeddings: Option<EmbeddingSettings>,
    /// From `~/.kiri/` or the `--instructions` override — a path the user typed, so trusted alike.
    /// Rendered as authoritative guidance.
    pub instructions_global: Option<String>,
    /// From the workspace root, which a third-party repo may have authored: rendered as untrusted
    /// guidance, never an authoritative directive (S3-1).
    pub instructions_project: Option<String>,
    /// In discovery order, for TUI display.
    pub instruction_paths: Vec<PathBuf>,
}

/// An existing provider id whose endpoint/credential to reuse, plus the embeddings model id.
#[derive(Debug, Clone)]
pub struct EmbeddingSettings {
    pub provider_id: String,
    pub model: String,
}

/// Bounds a single read so an oversized file — or a symlink to an endless device reached via the explicit
/// `--instructions` override, which bypasses `find_instructions`'s guard by design — cannot hang or
/// exhaust memory during config resolve.
const MAX_INSTRUCTIONS_BYTES: u64 = 256 * 1024;

/// Open a path for capped read without following a final-component symlink when the OS allows it (#57).
/// On Unix uses `O_NOFOLLOW`; on Windows re-checks `symlink_metadata` after open (narrow residual race).
fn open_regular_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Linux 0x20000 / macOS & BSD 0x100 — `libc::O_NOFOLLOW` without a libc dependency.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const O_NOFOLLOW: i32 = 0x20000;
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        const O_NOFOLLOW: i32 = 0x0000_0100;
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )))]
        const O_NOFOLLOW: i32 = 0;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "instructions path is not a regular file",
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        // Residual: no portable O_NOFOLLOW. Open then re-stat the path; still refuse if it is a symlink.
        if std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "instructions path must not be a symlink",
            ));
        }
        let file = std::fs::File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "instructions path is not a regular file",
            ));
        }
        // Re-check after open (narrows #57 TOCTOU; not eliminated without nofollow).
        if std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "instructions path must not be a symlink",
            ));
        }
        Ok(file)
    }
}

fn read_capped(path: &std::path::Path) -> std::io::Result<String> {
    let file = open_regular_file(path)?;
    let mut buf = Vec::new();
    file.take(MAX_INSTRUCTIONS_BYTES).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Never a symlink: a hostile committed `CLAUDE.md` symlink must not redirect this read to `~/.ssh/id_rsa`
/// or another project's `.env`. Unlike the model's `read_file` tool, this pre-sandbox read never passes
/// through `FsSandbox`'s sensitive-path denylist.
fn is_regular_file(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// Discovery order: `KIRI.md` → `AGENTS.md` → `CLAUDE.md`.
pub(super) fn find_instructions(dir: &std::path::Path) -> Option<PathBuf> {
    ["KIRI.md", "AGENTS.md", "CLAUDE.md"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| is_regular_file(p))
}

/// Keeps the global (trusted) and project (untrusted, S3-1) layers separate so the system prompt can frame
/// each by its own trust level. A `--instructions` override replaces both and is global-tier: the user
/// typed that path, so it carries `~/.kiri`'s trust, not the workspace's.
fn load_instructions(
    workspace: &std::path::Path,
    global_dir: &std::path::Path,
    cli_override: Option<PathBuf>,
) -> Result<(Option<String>, Option<String>, Vec<PathBuf>)> {
    if let Some(path) = cli_override {
        let text = read_capped(&path)
            .map_err(|e| anyhow::anyhow!("--instructions: cannot read {}: {e}", path.display()))?;
        return Ok((Some(text), None, vec![path]));
    }
    // Best-effort: a read failure (permission denied, or the file vanished after the existence check)
    // skips this layer rather than aborting config resolve — instructions are optional, and the harness
    // must still boot.
    let load_layer = |dir: &std::path::Path| -> Option<(String, PathBuf)> {
        let p = find_instructions(dir)?;
        let text = read_capped(&p).ok()?;
        (!text.trim().is_empty()).then_some((text, p))
    };
    let global = load_layer(global_dir);
    let project = load_layer(workspace);
    let mut paths = Vec::new();
    paths.extend(global.as_ref().map(|(_, p)| p.clone()));
    paths.extend(project.as_ref().map(|(_, p)| p.clone()));
    Ok((global.map(|(t, _)| t), project.map(|(t, _)| t), paths))
}

impl Settings {
    /// Reduce the layered TOML config (`~/.kiri` global ← `<workspace>/.kiri` project) to `Settings`.
    /// `main` owns CLI parsing, so it can dispatch the headless `kiri sync` route before reaching the TUI.
    /// A first run with no config seeds a default NVIDIA provider and writes a starter `config.toml`.
    pub fn resolve(
        cli_path: Option<PathBuf>,
        cli_prompt: Option<String>,
        cli_instructions: Option<PathBuf>,
    ) -> Result<Self> {
        let path = cli_path.unwrap_or_else(|| PathBuf::from("."));

        let global_dir = kiri_global_dir();
        // Keep the kiri dir owner-only so the non-secret config.toml (co-located with credentials.json)
        // is not world-readable. Best-effort, but surfaced: a pre-existing `0755` dir that cannot be
        // coerced down is a real security signal — warn rather than swallow it — while still booting.
        if let Err(error) = ensure_private_dir(&global_dir) {
            eprintln!(
                "kiri: warning: could not make {} owner-only ({error}); it may be world-readable",
                global_dir.display()
            );
        }
        let global_path = global_dir.join("config.toml");
        let project_path = path.join(".kiri").join("config.toml");
        let had_global = global_path.exists();
        // Provider routing and security policy come from the trusted global config only; the workspace
        // (project) layer contributes only the `effort` preference. See `resolve_layers`.
        let (config, effort) = resolve_layers(
            read_config_file(&global_path)?,
            read_project_config_lenient(&project_path),
        );

        let (mut providers, mut active) =
            resolve_providers(config.providers, config.active_provider);
        // First run with no global config: seed the default provider and persist a starter file so the
        // user has something to edit. Best-effort — a write failure must not block the session.
        if providers.is_empty() {
            let default = default_provider();
            active = default.id.clone();
            providers.push(default);
            if !had_global
                && let Err(error) = write_starter_config(&global_path, &providers, &active)
            {
                eprintln!(
                    "kiri: could not write a starter config at {} ({error}); continuing",
                    global_path.display()
                );
            }
        }

        let (sandbox_enabled, require_confinement) =
            resolve_sandbox_mode(config.sandbox.mode.as_deref());
        let docs_path = config
            .paths
            .docs
            .map(|d| expand_home(&d))
            .or_else(|| std::env::var_os("KIRI_DOCS_PATH").map(PathBuf::from))
            .unwrap_or_else(|| path.join("docs"));

        let (loaded_instructions_global, loaded_instructions_project, loaded_paths) =
            load_instructions(&path, &global_dir, cli_instructions)?;

        Ok(Self {
            path,
            seed: cli_prompt,
            checkpoint_budget: TOOL_CHECKPOINT,
            max_tool_calls: MAX_TOOL_CALLS_PER_CHECKPOINT,
            plan_allow: compile_patterns("KIRI_PLAN_ALLOW", DEFAULT_PLAN_ALLOW)?,
            sandbox_enabled,
            require_confinement,
            sandbox_network: resolve_sandbox_network(config.sandbox.network.as_deref()),
            extra_ro: load_extra_paths("KIRI_SANDBOX_RO_PATHS", &[]),
            extra_rw: load_extra_paths("KIRI_SANDBOX_RW_PATHS", DEFAULT_RW_DIRS),
            connect_timeout: resolve_timeout(
                config.http.connect_timeout_ms,
                "KIRI_HTTP_CONNECT_TIMEOUT_MS",
                HTTP_CONNECT_TIMEOUT,
            ),
            read_timeout: resolve_timeout(
                config.http.read_timeout_ms,
                "KIRI_HTTP_READ_TIMEOUT_MS",
                HTTP_READ_TIMEOUT,
            ),
            thinking: resolve_bool(config.behavior.thinking, "KIRI_THINKING", true),
            memory_enabled: resolve_bool(config.behavior.memory, "KIRI_MEMORY", true),
            docs_path,
            shared_memory_db: global_dir.join("memory").join("shared.db"),
            sessions_db: global_dir.join("sessions.db"),
            credentials_file: global_dir.join("credentials.json"),
            global_dir: global_dir.clone(),
            config_path: global_path,
            providers,
            active_provider: active,
            effort,
            embeddings: match (config.embeddings.provider, config.embeddings.model) {
                (Some(provider), Some(model))
                    if !provider.trim().is_empty() && !model.trim().is_empty() =>
                {
                    Some(EmbeddingSettings {
                        provider_id: provider,
                        model,
                    })
                }
                _ => None,
            },
            instructions_global: loaded_instructions_global,
            instructions_project: loaded_instructions_project,
            instruction_paths: loaded_paths,
        })
    }

    /// The instructions text formatted for TUI display: paths header followed by both layers' content
    /// (global first, then project) — display-only, so it merges freely unlike the trust-separated
    /// system-prompt blocks.
    pub fn instructions_display(&self) -> Option<String> {
        if self.instructions_global.is_none() && self.instructions_project.is_none() {
            return None;
        }
        let header = self
            .instruction_paths
            .iter()
            .map(|p| format!("- {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        let text = [&self.instructions_global, &self.instructions_project]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");
        Some(format!("Arquivos carregados:\n{header}\n\n{text}"))
    }

    /// The active provider profile, resolved against the catalog. Errors if the active id names no
    /// configured provider (a corrupted config) — surfaced clearly rather than panicking.
    pub fn active_profile(&self) -> Result<&ProviderProfile> {
        self.providers
            .iter()
            .find(|p| p.id == self.active_provider)
            .ok_or_else(|| {
                anyhow!(
                    "active provider '{}' is not configured",
                    self.active_provider
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &std::path::Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn find_instructions_returns_none_when_absent() {
        let dir = TempDir::new().unwrap();
        assert!(find_instructions(dir.path()).is_none());
    }

    #[test]
    fn find_instructions_prefers_kiri_over_agents_over_claude() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "CLAUDE.md", "claude");
        write(dir.path(), "AGENTS.md", "agents");
        write(dir.path(), "KIRI.md", "kiri");
        assert_eq!(
            find_instructions(dir.path()).unwrap().file_name().unwrap(),
            "KIRI.md"
        );
    }

    #[test]
    fn find_instructions_falls_back_agents_then_claude() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "CLAUDE.md", "claude");
        write(dir.path(), "AGENTS.md", "agents");
        assert_eq!(
            find_instructions(dir.path()).unwrap().file_name().unwrap(),
            "AGENTS.md",
            "AGENTS.md must win over CLAUDE.md when KIRI.md is absent"
        );

        std::fs::remove_file(dir.path().join("AGENTS.md")).unwrap();
        assert_eq!(
            find_instructions(dir.path()).unwrap().file_name().unwrap(),
            "CLAUDE.md",
            "CLAUDE.md is the last fallback"
        );
    }

    #[test]
    fn load_instructions_with_no_files_returns_none() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let (global_text, project_text, paths) =
            load_instructions(workspace.path(), global.path(), None).unwrap();
        assert!(global_text.is_none());
        assert!(project_text.is_none());
        assert!(paths.is_empty());
    }

    #[test]
    fn load_instructions_uses_only_global_when_project_absent() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        write(global.path(), "CLAUDE.md", "global rules");
        let (global_text, project_text, paths) =
            load_instructions(workspace.path(), global.path(), None).unwrap();
        assert_eq!(global_text.unwrap(), "global rules");
        assert!(project_text.is_none());
        assert_eq!(paths, vec![global.path().join("CLAUDE.md")]);
    }

    #[test]
    fn load_instructions_keeps_global_and_project_separate() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        write(global.path(), "CLAUDE.md", "global rules");
        write(workspace.path(), "CLAUDE.md", "project rules");
        let (global_text, project_text, paths) =
            load_instructions(workspace.path(), global.path(), None).unwrap();
        assert_eq!(
            global_text.unwrap(),
            "global rules",
            "global layer must never merge with the untrusted project layer (S3-1)"
        );
        assert_eq!(project_text.unwrap(), "project rules");
        assert_eq!(
            paths,
            vec![
                global.path().join("CLAUDE.md"),
                workspace.path().join("CLAUDE.md"),
            ]
        );
    }

    #[test]
    fn load_instructions_skips_a_blank_layer() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        write(global.path(), "CLAUDE.md", "   \n  ");
        write(workspace.path(), "CLAUDE.md", "project rules");
        let (global_text, project_text, paths) =
            load_instructions(workspace.path(), global.path(), None).unwrap();
        assert!(global_text.is_none());
        assert_eq!(project_text.unwrap(), "project rules");
        assert_eq!(paths, vec![workspace.path().join("CLAUDE.md")]);
    }

    #[test]
    fn cli_override_replaces_both_layers() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let override_dir = TempDir::new().unwrap();
        write(global.path(), "CLAUDE.md", "global rules");
        write(workspace.path(), "CLAUDE.md", "project rules");
        write(override_dir.path(), "custom.md", "override rules");
        let override_path = override_dir.path().join("custom.md");

        let (global_text, project_text, paths) =
            load_instructions(workspace.path(), global.path(), Some(override_path.clone()))
                .unwrap();
        assert_eq!(
            global_text.unwrap(),
            "override rules",
            "a CLI override is user-typed, so it is treated as global-tier trust"
        );
        assert!(project_text.is_none());
        assert_eq!(paths, vec![override_path]);
    }

    #[test]
    fn cli_override_of_a_missing_file_errors() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let missing = workspace.path().join("nope.md");
        assert!(load_instructions(workspace.path(), global.path(), Some(missing)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn find_instructions_skips_a_symlinked_candidate() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        // A secret outside the searched directory, reachable only via a symlink placed inside it under
        // a candidate name — must never be followed by the auto-discovery.
        let outside = TempDir::new().unwrap();
        write(outside.path(), "secret.md", "outside secret");
        symlink(
            outside.path().join("secret.md"),
            dir.path().join("CLAUDE.md"),
        )
        .unwrap();

        assert!(
            find_instructions(dir.path()).is_none(),
            "a symlinked candidate must never be treated as found"
        );
    }

    #[test]
    fn load_instructions_caps_an_oversized_file() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let oversized = "a".repeat(MAX_INSTRUCTIONS_BYTES as usize + 1024);
        write(global.path(), "CLAUDE.md", &oversized);

        let (global_text, _, _) = load_instructions(workspace.path(), global.path(), None).unwrap();
        assert_eq!(
            global_text.unwrap().len(),
            MAX_INSTRUCTIONS_BYTES as usize,
            "an oversized instructions file must be truncated at the byte cap, not read in full"
        );
    }
}
