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

/// The kiri global config/state directory (`~/.kiri`). Houses `config.toml`, the credentials fallback
/// file, and the shared-memory database. Private: only `Settings::resolve` (this file) derives it; every
/// consumer reads the resolved `Settings::global_dir` field, the single harness-home source (ADR 0015).
fn kiri_global_dir() -> PathBuf {
    expand_home("~/.kiri")
}

/// Load the optional `~/.kiri/.env` into process env before config resolution, so a user can keep API
/// keys (and other trusted overrides) in one owner-only file that seeds `credentials.json`. Read ONLY
/// from the trusted global dir, never the cwd — a hostile project repo must not be able to inject env
/// and thereby redirect a credential or weaken the sandbox (ADR 0020; the "project layer is untrusted"
/// invariant). Best-effort: an absent or malformed `.env` just means no vars are set, never a boot
/// failure, and `dotenvy` never overrides an already-exported var.
pub fn load_global_env() {
    let env_path = kiri_global_dir().join(".env");
    // Deliberately ignored: `.env` is an optional convenience. A missing file, or a malformed line that
    // fails to parse, must not abort boot — the affected key simply stays unset and onboarding handles it.
    let _ = dotenvy::from_path(&env_path);
}

/// The resolved configuration the composition root needs to wire the harness. Provider endpoints and
/// the active model come from the configured [`ProviderProfile`] catalog; the matching secret is
/// fetched from the credential store at wire time (never stored here).
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
    /// HTTP client timeouts for the provider: `connect_timeout` caps connection setup, `read_timeout`
    /// caps idle time between received bytes (streaming-safe). Bound a hung provider so a turn fails
    /// fast with a clear error instead of hanging silently.
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    /// Ask the model to stream reasoning. On by default; disable for a model that rejects/stalls on it.
    pub thinking: bool,
    /// Whether the memory contexts (project + shared) and the docs/memory tools are wired.
    pub memory_enabled: bool,
    /// The project's documentation root that `consult_docs` searches. Defaults to `<path>/docs`.
    pub docs_path: PathBuf,
    /// The cross-project shared memory database. Defaults to `~/.kiri/memory/shared.db`.
    pub shared_memory_db: PathBuf,
    /// The persisted-conversations database. Defaults to `~/.kiri/sessions.db`. Gated by `memory_enabled`.
    pub sessions_db: PathBuf,
    /// The credential-store file. `~/.kiri/credentials.json`.
    pub credentials_file: PathBuf,
    /// The harness home (`~/.kiri`). The single source every consumer reads instead of re-deriving it
    /// from `config_path.parent()`; the sync work-tree lives at `<global_dir>/sync`.
    pub global_dir: PathBuf,
    /// The global config file (`~/.kiri/config.toml`). The runtime writes live `/models`/`/effort`
    /// changes back here (the trusted layer only).
    pub config_path: PathBuf,
    /// The configured provider catalog (non-secret). The user selects among these via `/provider`.
    pub providers: Vec<ProviderProfile>,
    /// The id of the active provider — must name one of `providers`.
    pub active_provider: String,
    /// The reasoning/output effort dial, mapped per provider by its adapter.
    pub effort: Effort,
    /// Optional embeddings config for semantic recall: which configured provider to reuse and the model.
    /// `None` keeps recall keyword-only. Trusted (global) layer only.
    pub embeddings: Option<EmbeddingSettings>,
    /// The merged instructions text to inject into the system prompt (global + project, or a CLI
    /// override). `None` when no instructions file was found.
    pub instructions: Option<String>,
    /// The file paths that contributed to `instructions`, in discovery order, for TUI display.
    pub instruction_paths: Vec<PathBuf>,
}

/// Resolved `[embeddings]` config: an existing provider id whose endpoint/credential to reuse, and the
/// embeddings model id.
#[derive(Debug, Clone)]
pub struct EmbeddingSettings {
    pub provider_id: String,
    pub model: String,
}

/// Largest instructions file read into memory, mirroring `docs_library`'s `MAX_FILE_BYTES` /
/// `file_project_memory`'s `MAX_ENTRY_BYTES` — bounds a single read so an oversized committed file, or a
/// symlink to an endless device that slipped past `find_instructions`'s guard (the explicit
/// `--instructions` override does not go through that guard, by design — it names a path the user typed
/// themselves), cannot hang or exhaust memory during config resolve.
const MAX_INSTRUCTIONS_BYTES: u64 = 256 * 1024;

/// Read at most `MAX_INSTRUCTIONS_BYTES` of `path` as lossy UTF-8.
fn read_capped(path: &std::path::Path) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take(MAX_INSTRUCTIONS_BYTES).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// True only for a regular file, never a symlink — mirrors `docs_library`'s directory-walk guard. A
/// hostile committed symlink named e.g. `CLAUDE.md` must not redirect this harness-internal, pre-sandbox
/// read to a file outside the project (`~/.ssh/id_rsa`, another project's `.env`, …): unlike the model's
/// own `read_file` tool, this read never passes through `FsSandbox`'s sensitive-path denylist.
fn is_regular_file(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// Return the first instructions file found in `dir` by the discovery order `KIRI.md` → `AGENTS.md` →
/// `CLAUDE.md`. Returns `None` if none of the candidates exist (or exist only as a symlink).
pub(super) fn find_instructions(dir: &std::path::Path) -> Option<PathBuf> {
    ["KIRI.md", "AGENTS.md", "CLAUDE.md"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| is_regular_file(p))
}

/// Discover and load instructions, merging global (`~/.kiri/`) and project (workspace root) layers.
/// A CLI override (`--instructions`) replaces both layers. Returns `(merged_text, contributing_paths)`.
fn load_instructions(
    workspace: &std::path::Path,
    global_dir: &std::path::Path,
    cli_override: Option<PathBuf>,
) -> Result<(Option<String>, Vec<PathBuf>)> {
    if let Some(path) = cli_override {
        let text = read_capped(&path)
            .map_err(|e| anyhow::anyhow!("--instructions: cannot read {}: {e}", path.display()))?;
        return Ok((Some(text), vec![path]));
    }
    let mut parts: Vec<String> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for dir in [global_dir, workspace] {
        // Best-effort: a read failure (permission denied, or the file vanished after the existence
        // check) skips this layer rather than aborting config resolve — instructions are optional, and
        // the harness must still boot.
        if let Some(p) = find_instructions(dir)
            && let Ok(text) = read_capped(&p)
            && !text.trim().is_empty()
        {
            parts.push(text);
            paths.push(p);
        }
    }
    let merged = if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    };
    Ok((merged, paths))
}

impl Settings {
    /// Resolve the runtime settings from the already-parsed CLI path/prompt: load the layered TOML
    /// config (`~/.kiri` global ← `<workspace>/.kiri` project) and reduce it to `Settings`. `main` owns
    /// CLI parsing — so it can dispatch the headless `kiri sync` route before reaching the TUI — and
    /// hands the values here. The harness owns its config (TOML) and secrets (the 0600 credentials file); a
    /// first run with no config seeds a default NVIDIA provider and writes a starter `~/.kiri/config.toml`.
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

        let (loaded_instructions, loaded_paths) =
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
            instructions: loaded_instructions,
            instruction_paths: loaded_paths,
        })
    }

    /// The instructions text formatted for TUI display: paths header followed by the merged content.
    /// Returns `None` when no instructions were loaded.
    pub fn instructions_display(&self) -> Option<String> {
        let text = self.instructions.as_deref()?;
        let header = self
            .instruction_paths
            .iter()
            .map(|p| format!("- {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
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
        let (text, paths) = load_instructions(workspace.path(), global.path(), None).unwrap();
        assert!(text.is_none());
        assert!(paths.is_empty());
    }

    #[test]
    fn load_instructions_uses_only_global_when_project_absent() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        write(global.path(), "CLAUDE.md", "global rules");
        let (text, paths) = load_instructions(workspace.path(), global.path(), None).unwrap();
        assert_eq!(text.unwrap(), "global rules");
        assert_eq!(paths, vec![global.path().join("CLAUDE.md")]);
    }

    #[test]
    fn load_instructions_appends_project_after_global() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        write(global.path(), "CLAUDE.md", "global rules");
        write(workspace.path(), "CLAUDE.md", "project rules");
        let (text, paths) = load_instructions(workspace.path(), global.path(), None).unwrap();
        assert_eq!(
            text.unwrap(),
            "global rules\n\nproject rules",
            "global must come first, project appended after a blank line"
        );
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
        let (text, paths) = load_instructions(workspace.path(), global.path(), None).unwrap();
        assert_eq!(text.unwrap(), "project rules");
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

        let (text, paths) =
            load_instructions(workspace.path(), global.path(), Some(override_path.clone()))
                .unwrap();
        assert_eq!(text.unwrap(), "override rules");
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

        let (text, _) = load_instructions(workspace.path(), global.path(), None).unwrap();
        assert_eq!(
            text.unwrap().len(),
            MAX_INSTRUCTIONS_BYTES as usize,
            "an oversized instructions file must be truncated at the byte cap, not read in full"
        );
    }
}
