use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, anyhow};

use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{AuthMethod, Effort, ProviderKind, ProviderProfile};

use super::defaults::DEFAULT_PROVIDER_ID;
use super::raw::{RawConfig, read_config_file};

/// The single private-`~/.kiri`-dir creator: every such creation routes through this `0700` helper, never
/// a plain `0755` `create_dir_all`, so the non-secret files there are not world-readable. On Windows the
/// inherited user-profile DACL is the equivalent.
#[cfg(unix)]
pub(crate) fn ensure_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    // Coerce an already-existing dir (e.g. created `0755` by an earlier version) down to `0700`.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(crate) fn ensure_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Read-modify-write, preserving every other section. Only the trusted global config is written, never the
/// untrusted project layer, which would let a workspace change provider routing (see `resolve_layers`).
/// TOML comments in a hand-edited file are dropped on rewrite; the values survive.
fn update_global_config(
    config_path: &Path,
    mutate: impl FnOnce(&mut RawConfig),
) -> Result<(), AgentError> {
    let mut config =
        read_config_file(config_path).map_err(|e| AgentError::Config(e.to_string()))?;
    mutate(&mut config);
    let body = toml::to_string_pretty(&config)
        .map_err(|e| AgentError::Config(format!("failed to encode config: {e}")))?;
    if let Some(parent) = config_path.parent() {
        ensure_private_dir(parent).map_err(|e| {
            AgentError::Config(format!("failed to create {}: {e}", parent.display()))
        })?;
    }
    // DATA-01: a plain truncate-then-write could leave the boot-critical `config.toml` truncated on a
    // crash, and the fail-fast global loader would then abort the next boot.
    crate::shared::infra::fs::write_atomic_sync(config_path, body.as_bytes()).map_err(|e| {
        AgentError::Config(format!(
            "failed to write config at {}: {e}",
            config_path.display()
        ))
    })
}

/// Persist a live `/models` change: set the active model on its provider and add it to that provider's
/// catalog if missing. A no-op if the provider id is not in the config (the live change still stands).
pub fn persist_active_model(
    config_path: &Path,
    provider_id: &str,
    model: &str,
) -> Result<(), AgentError> {
    update_global_config(config_path, |config| {
        if let Some(profile) = config.providers.get_mut(provider_id) {
            profile.model = model.to_string();
            if !profile.models.iter().any(|m| m == model) {
                profile.models.push(model.to_string());
            }
        }
    })
}

/// Persist a live `/effort` change to the global config.
pub fn persist_effort(config_path: &Path, effort: Effort) -> Result<(), AgentError> {
    update_global_config(config_path, |config| config.effort = Some(effort))
}

/// Persist a live `/provider` switch (the active provider id) to the global config.
pub fn persist_active_provider(config_path: &Path, provider_id: &str) -> Result<(), AgentError> {
    update_global_config(config_path, |config| {
        config.active_provider = Some(provider_id.to_string())
    })
}

/// Add or replace a provider profile in the global config (from the add wizard). The profile's `id`
/// keys the table (and is itself `#[serde(skip)]` in the body); the secret material is stored separately
/// in the credential store (the `0600` `credentials.json`), never here.
pub fn upsert_provider(config_path: &Path, profile: &ProviderProfile) -> Result<(), AgentError> {
    update_global_config(config_path, |config| {
        config.providers.insert(profile.id.clone(), profile.clone());
    })
}

/// Remove a provider from the global config. If it was the active provider, the active selection is
/// cleared (the runtime then falls back to the first remaining provider or enters onboarding).
pub fn delete_provider(config_path: &Path, id: &str) -> Result<(), AgentError> {
    update_global_config(config_path, |config| {
        config.providers.remove(id);
        if config.active_provider.as_deref() == Some(id) {
            config.active_provider = config.providers.keys().next().cloned();
        }
    })
}

/// The default first-run provider: NVIDIA's OpenAI-compatible endpoint with the model taken from a
/// legacy `NVIDIA_MODEL` env var if present (one-time migration aid), else left blank for the user to
/// fill via `/provider`.
pub(super) fn default_provider() -> ProviderProfile {
    let model = std::env::var("NVIDIA_MODEL").unwrap_or_default();
    let models = if model.is_empty() {
        Vec::new()
    } else {
        vec![model.clone()]
    };
    ProviderProfile {
        id: DEFAULT_PROVIDER_ID.to_string(),
        kind: ProviderKind::Nvidia,
        base_url: ProviderKind::Nvidia.default_base_url().to_string(),
        model,
        models,
        auth: AuthMethod::ApiKey,
        thinking: None,
    }
}

/// Write a starter global config so a first-run user has a real file to edit. Best-effort.
pub(super) fn write_starter_config(
    path: &Path,
    providers: &[ProviderProfile],
    active: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let table: BTreeMap<String, ProviderProfile> = providers
        .iter()
        .map(|p| (p.id.clone(), p.clone()))
        .collect();
    let config = RawConfig {
        active_provider: Some(active.to_string()),
        effort: Some(Effort::default()),
        providers: table,
        ..RawConfig::default()
    };
    let body = toml::to_string_pretty(&config)
        .map_err(|e| anyhow!("failed to serialize starter config: {e}"))?;
    // DATA-01: atomic write (see `update_global_config`) so a crash mid-write never leaves a truncated
    // boot-critical config behind.
    crate::shared::infra::fs::write_atomic_sync(path, body.as_bytes())
        .map_err(|e| anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_config_writers_preserve_every_other_section() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
active_provider = "nvidia"
effort = "high"

[providers.nvidia]
kind = "nvidia"
base_url = "https://integrate.api.nvidia.com/v1"
model = "m1"
models = ["m1"]
auth = "api-key"

[sandbox]
mode = "require"

[http]
read_timeout_ms = 99000
"#,
        )
        .unwrap();

        persist_effort(&path, Effort::Max).unwrap();
        persist_active_model(&path, "nvidia", "m2").unwrap();
        let claude = ProviderProfile {
            id: "claude".into(),
            kind: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-opus-4-8".into(),
            models: vec!["claude-opus-4-8".into()],
            auth: AuthMethod::ApiKey,
            thinking: None,
        };
        upsert_provider(&path, &claude).unwrap();
        persist_active_provider(&path, "claude").unwrap();

        let config = read_config_file(&path).unwrap();
        assert_eq!(config.effort, Some(Effort::Max));
        assert_eq!(config.active_provider.as_deref(), Some("claude"));
        // The model write updated the active model AND extended the catalog.
        let nvidia = config.providers.get("nvidia").expect("nvidia preserved");
        assert_eq!(nvidia.model, "m2");
        assert!(nvidia.models.iter().any(|m| m == "m2"));
        // The upserted provider is present and keyed by id (its `id` field is `#[serde(skip)]`).
        assert!(config.providers.contains_key("claude"));
        // Non-targeted sections survived the read-modify-write (not lossy).
        assert_eq!(config.sandbox.mode.as_deref(), Some("require"));
        assert_eq!(config.http.read_timeout_ms, Some(99000));
    }

    // The writers run deep in the live `/models`/`/effort`/`/provider` handlers, so a failure must come
    // back as a typed `AgentError::Config` (surfaced as a transcript Notice), never an anyhow panic.
    #[test]
    fn persist_active_model_returns_agenterror_config_on_unwritable_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        // A config path nested under a regular file: neither the read nor the owner-only dir creation can
        // succeed, so the writer must surface the failure as `AgentError::Config`.
        let bad = file.join("sub").join("config.toml");
        assert!(matches!(
            persist_active_model(&bad, "nvidia", "m"),
            Err(AgentError::Config(_))
        ));
    }

    #[test]
    fn config_writes_are_atomic_no_temp_sibling_lingers() {
        // DATA-01: the live writers must go through the atomic temp-then-rename path, so no temp sibling
        // lingers after a successful write and a crash can never leave config.toml truncated.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        persist_effort(&path, Effort::High).unwrap();
        assert!(path.exists());
        assert!(
            !dir.path().join(".config.toml.kiri-tmp").exists(),
            "the atomic write must consume the temp sibling"
        );
    }

    #[test]
    fn persist_effort_round_trips_and_is_ok() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        persist_effort(&path, Effort::High).unwrap();
        let config = read_config_file(&path).unwrap();
        assert_eq!(config.effort, Some(Effort::High));
    }

    #[test]
    fn upsert_provider_adds_then_persist_active_provider_chains() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let claude = ProviderProfile {
            id: "claude".into(),
            kind: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-opus-4-8".into(),
            models: vec!["claude-opus-4-8".into()],
            auth: AuthMethod::ApiKey,
            thinking: None,
        };
        // Mirror the runtime's `.and_then` chain (upsert then activate) under the new error type.
        upsert_provider(&path, &claude)
            .and_then(|()| persist_active_provider(&path, "claude"))
            .unwrap();
        let config = read_config_file(&path).unwrap();
        assert!(config.providers.contains_key("claude"));
        assert_eq!(config.active_provider.as_deref(), Some("claude"));
    }

    #[test]
    fn delete_provider_removes_it_and_clears_active_when_it_was_active() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let nvidia = ProviderProfile {
            id: "nvidia".into(),
            kind: ProviderKind::Nvidia,
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            model: "m".into(),
            models: vec![],
            auth: AuthMethod::ApiKey,
            thinking: None,
        };
        upsert_provider(&path, &nvidia).unwrap();
        persist_active_provider(&path, "nvidia").unwrap();
        delete_provider(&path, "nvidia").unwrap();
        let config = read_config_file(&path).unwrap();
        assert!(!config.providers.contains_key("nvidia"));
        // active_provider was pointing at "nvidia"; after delete it must be cleared or point elsewhere.
        assert_ne!(config.active_provider.as_deref(), Some("nvidia"));
    }

    // Compile-asserting regression lock: the writers expose the typed signature, not anyhow.
    #[test]
    fn config_writers_do_not_return_anyhow() {
        let _: fn(&Path, Effort) -> Result<(), AgentError> = persist_effort;
        let _: fn(&Path, &str, &str) -> Result<(), AgentError> = persist_active_model;
        let _: fn(&Path, &str) -> Result<(), AgentError> = persist_active_provider;
        let _: fn(&Path, &ProviderProfile) -> Result<(), AgentError> = upsert_provider;
        let _: fn(&Path, &str) -> Result<(), AgentError> = delete_provider;
    }

    #[cfg(unix)]
    #[test]
    fn update_global_config_creates_owner_only_dir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let kiri_dir = tmp.path().join("sub").join(".kiri");
        let config_path = kiri_dir.join("config.toml");
        // Any global-config writer creates the parent through `ensure_private_dir`.
        persist_effort(&config_path, Effort::High).unwrap();
        assert!(config_path.exists());
        let mode = std::fs::metadata(&kiri_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "created kiri dir must be 0700, got {mode:o}");
    }
}
