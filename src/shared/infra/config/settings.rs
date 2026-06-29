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
    compile_patterns, expand_home, load_extra_paths, load_net_allow, resolve_bool,
    resolve_sandbox_mode, resolve_sandbox_network, resolve_timeout,
};
use super::writers::{default_provider, ensure_private_dir, write_starter_config};

/// The kiri global config/state directory (`~/.kiri`). Houses `config.toml`, the credentials fallback
/// file, and the shared-memory database. Private: only `Settings::resolve` (this file) derives it; every
/// consumer reads the resolved `Settings::global_dir` field, the single harness-home source (ADR 0015).
fn kiri_global_dir() -> PathBuf {
    expand_home("~/.kiri")
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
    /// Base network stance for `run_command` (the dev-command allow-list may widen it per call).
    pub sandbox_network: NetworkPolicy,
    /// Commands allowed to reach the network under confinement (dev / package-manager tools).
    pub net_allow: Arc<[Regex]>,
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
    /// The credential-store fallback file when no OS keyring is reachable. `~/.kiri/credentials.json`.
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

/// Return the first instructions file found in `dir` by the discovery order `KIRI.md` → `AGENTS.md` →
/// `CLAUDE.md`. Returns `None` if none of the candidates exist.
pub(super) fn find_instructions(dir: &std::path::Path) -> Option<PathBuf> {
    ["KIRI.md", "AGENTS.md", "CLAUDE.md"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
}

/// Discover and load instructions, merging global (`~/.kiri/`) and project (workspace root) layers.
/// A CLI override (`--instructions`) replaces both layers. Returns `(merged_text, contributing_paths)`.
fn load_instructions(
    workspace: &std::path::Path,
    global_dir: &std::path::Path,
    cli_override: Option<PathBuf>,
) -> Result<(Option<String>, Vec<PathBuf>)> {
    if let Some(path) = cli_override {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("--instructions: cannot read {}: {e}", path.display()))?;
        return Ok((Some(text), vec![path]));
    }
    let mut parts: Vec<String> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for dir in [global_dir, workspace] {
        if let Some(p) = find_instructions(dir)
            && let Ok(text) = std::fs::read_to_string(&p)
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
    /// hands the values here. No `.env`: the harness owns its config (TOML) and secrets (keyring); a
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
            net_allow: load_net_allow()?,
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
