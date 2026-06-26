use std::io::IsTerminal;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::modules::agent::application::agent_loop::AgentLoop;
use crate::modules::memory::application::memory_port::{MemoryPort, MemoryPortImpl};
use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::memory::domain::project_memory::{
    ProjectMemory, SharedMemory, project_id_from_path,
};
use crate::modules::memory::infrastructure::docs_library::DocsLibrary;
use crate::modules::memory::infrastructure::file_project_memory::FileProjectMemory;
use crate::modules::memory::infrastructure::file_project_store::FileProjectStore;
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::modules::memory::infrastructure::sqlite_shared_store::SqliteSharedStore;
use crate::modules::memory::infrastructure::tools::default_memory_tools;
use crate::modules::provider::application::secret_store::SecretStore;
use crate::modules::provider::infrastructure::factory::build_provider;
use crate::modules::provider::infrastructure::secrets::default_secret_store;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::application::tool::Tool;
use crate::modules::tools::infrastructure::confine;
use crate::modules::tools::infrastructure::control::present_plan::PresentPlan;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tui::infrastructure::runtime::{ProviderSwap, Tui};
use crate::shared::infra::config::Settings;
use crate::shared::kernel::provider::{
    AuthMethod, Credential, ProviderKind, ProviderProfile, Secret,
};

/// Caps for the start-of-session memory digest injected into the system prompt: how many entries to
/// pull per scope and the total byte budget, so the prompt stays bounded regardless of memory size.
const DIGEST_PROJECT_CAP: usize = 12;
const DIGEST_SHARED_CAP: usize = 12;
const MAX_DIGEST_BYTES: usize = 4096;

/// The composition root: build the sandbox, the provider adapter, the tool registry and the agent loop
/// from resolved settings, then assemble the full-screen TUI. This is the one place adapters are chosen.
/// The TUI requires an interactive terminal; a non-TTY stdout (piped output, CI) fails fast.
pub async fn wire(settings: Settings) -> Result<Tui> {
    if !std::io::stdout().is_terminal() {
        bail!("Kiri requires an interactive terminal (stdout is not a TTY)");
    }
    // Memory & docs first: a degraded store (init failure) is surfaced and left inert, never fatal.
    let (memory_tools, memory_digest) = build_memory(&settings).await?;
    let confiner = confine::default_command_sandbox(settings.sandbox_enabled);
    let sandbox = Sandbox::with_confinement(
        &settings.path,
        settings.sensitive.clone(),
        confiner,
        settings.sandbox_network,
        settings.extra_ro.clone(),
        settings.extra_rw.clone(),
    )?;
    // A timed client: a hung provider must fail fast with a clear error, never hang the turn forever.
    // `read_timeout` caps idle time between received bytes, so it bounds a stalled stream without
    // killing a long but active one.
    let client = reqwest::Client::builder()
        .connect_timeout(settings.connect_timeout)
        .read_timeout(settings.read_timeout)
        .build()
        .context("failed to build the HTTP client")?;
    // Resolve the active provider profile and its credential (OS keyring, or a 0600 fallback file),
    // then select the adapter. This is the one place adapters are chosen.
    let secrets = default_secret_store(settings.credentials_file.clone());
    let profile = settings.active_profile()?.clone();
    let credential = resolve_credential(&profile, secrets.as_ref())?;
    // Build the initial adapter; the client + credential are cloned so the runtime keeps a `ProviderSwap`
    // able to rebuild the adapter on a live `/effort` change without a keyring round-trip.
    let provider = build_provider(
        client.clone(),
        &profile,
        credential.clone(),
        settings.thinking,
        settings.effort,
    )?;
    // The file tools plus the plan-mode control tool. `present_plan` is advertised only in plan mode
    // (it carries `plan_only`); the registry's `schemas()` withholds it everywhere else.
    let mut tools = default_fs_tools(
        settings.plan_blacklist.clone(),
        settings.net_allow.clone(),
        settings.require_confinement,
    );
    tools.push(Box::new(PresentPlan));
    tools.extend(memory_tools);
    let registry = ToolRegistry::new(tools);
    let agent_loop = AgentLoop::new(
        provider,
        registry,
        profile.model.clone(),
        settings.checkpoint_budget,
        settings.max_tool_calls,
    );

    // The session's system prompt is the static base plus, when present, a digest of recalled memory.
    let system_prompt = if memory_digest.is_empty() {
        settings.system_prompt.to_string()
    } else {
        format!("{}\n\n{}", settings.system_prompt, memory_digest)
    };

    let provider_swap = ProviderSwap::new(
        client,
        profile,
        credential,
        settings.thinking,
        settings.effort,
    );
    Ok(Tui::new(
        agent_loop,
        sandbox,
        system_prompt,
        settings.seed,
        provider_swap,
        settings.config_path,
    ))
}

/// Wire the memory contexts (project file store + shared SQLite store) and the docs library, returning
/// the memory/docs tools and a start-of-session digest to inject into the system prompt. A store whose
/// `init` fails is surfaced on stderr and left inert (`is_available() == false`) rather than aborting:
/// memory is auxiliary, so the harness must still start. Returns no tools and an empty digest when
/// memory is disabled (`KIRI_MEMORY=off`).
async fn build_memory(settings: &Settings) -> Result<(Vec<Box<dyn Tool>>, String)> {
    if !settings.memory_enabled {
        return Ok((Vec::new(), String::new()));
    }
    let project_id = project_id_from_path(&settings.path);

    // Project memory: Markdown files under <workspace>/.kiri/memory.
    let project_memory = FileProjectMemory::new(settings.path.join(".kiri").join("memory"));
    let project_ok = match project_memory.init().await {
        Ok(()) => true,
        Err(error) => {
            eprintln!("kiri: project memory unavailable ({error}); continuing without it");
            false
        }
    };
    let project_entries = if project_ok {
        // The startup digest is best-effort: a listing failure must not block the session, so fall
        // back to an empty digest rather than aborting.
        project_memory
            .list(0, DIGEST_PROJECT_CAP)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Shared memory: a single SQLite database under ~/.kiri/memory. Fall back to an inert in-memory
    // database if the on-disk one cannot be opened.
    let (shared_memory, shared_ok) =
        match SqliteSharedMemory::new(settings.shared_memory_db.clone()) {
            Ok(memory) => match memory.init().await {
                Ok(()) => (memory, true),
                Err(error) => {
                    eprintln!("kiri: shared memory unavailable ({error}); continuing without it");
                    (memory, false)
                }
            },
            Err(error) => {
                eprintln!("kiri: shared memory unavailable ({error}); continuing without it");
                (SqliteSharedMemory::in_memory()?, false)
            }
        };
    let shared_entries = if shared_ok {
        // Best-effort digest: continue with an empty list instead of aborting if the query fails.
        shared_memory
            .list(0, DIGEST_SHARED_CAP)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let memory: Arc<dyn MemoryPort> = Arc::new(MemoryPortImpl::new(
        FileProjectStore::new(project_memory, project_ok),
        SqliteSharedStore::new(shared_memory, shared_ok),
    ));
    let docs = Arc::new(DocsLibrary::new(settings.docs_path.clone()));

    let tools = default_memory_tools(memory, docs, project_id);
    let digest = render_digest(&project_entries, &shared_entries);
    Ok((tools, digest))
}

/// Render the start-of-session memory digest: a bounded "# Relevant memory" section listing the most
/// recent project and shared entries, for grounding without spending the whole context window.
fn render_digest(project: &[MemoryEntry], shared: &[MemoryEntry]) -> String {
    if project.is_empty() && shared.is_empty() {
        return String::new();
    }
    let mut body = String::from(
        "# Relevant memory\nDurable knowledge recalled for this workspace. Use recall_memory and \
         consult_docs for more.\n",
    );
    let mut budget = MAX_DIGEST_BYTES;
    append_digest_section(&mut body, &mut budget, "## Project", project);
    append_digest_section(&mut body, &mut budget, "## Shared (cross-project)", shared);
    body
}

fn append_digest_section(
    body: &mut String,
    budget: &mut usize,
    title: &str,
    entries: &[MemoryEntry],
) {
    if entries.is_empty() {
        return;
    }
    body.push('\n');
    body.push_str(title);
    body.push('\n');
    for entry in entries {
        let rendered = entry.format_for_context();
        if rendered.len() + 1 > *budget {
            break;
        }
        *budget -= rendered.len() + 1;
        body.push_str(&rendered);
        body.push('\n');
    }
}

/// Resolve the active provider's credential: the stored one if present, else a one-time import from a
/// legacy env var (migration aid / CI escape hatch) for API-key providers, else a clear error telling
/// the user how to supply it. Never logs the secret.
fn resolve_credential(profile: &ProviderProfile, secrets: &dyn SecretStore) -> Result<Credential> {
    if let Some(credential) = secrets.get(&profile.id)? {
        return Ok(credential);
    }
    if profile.auth == AuthMethod::ApiKey
        && let Some(key) = api_key_from_env(profile)
    {
        let credential = Credential::ApiKey {
            key: Secret::new(key),
        };
        // Persist so later sessions don't need the env var. A store failure is non-fatal: use the key
        // for this session and tell the user it was not saved.
        match secrets.set(&profile.id, &credential) {
            Ok(()) => eprintln!(
                "kiri: imported API key for provider '{}' into the credential store",
                profile.id
            ),
            Err(error) => eprintln!(
                "kiri: could not persist the credential for '{}' ({error}); using it this session only",
                profile.id
            ),
        }
        return Ok(credential);
    }
    bail!(
        "no credential for provider '{}'. Set {} (one-time import) or configure it via /provider",
        profile.id,
        env_hint(profile)
    )
}

/// The legacy/CI env var an API-key provider can be primed from, by kind plus a generic per-id form.
fn api_key_from_env(profile: &ProviderProfile) -> Option<String> {
    let generic = generic_env_key(profile);
    let vendor: &[&str] = match profile.kind {
        ProviderKind::Nvidia => &["NVIDIA_API_KEY"],
        ProviderKind::Openai => &["OPENAI_API_KEY"],
        ProviderKind::Anthropic => &["ANTHROPIC_API_KEY"],
        ProviderKind::OpenAiCompatible | ProviderKind::Custom => &[],
    };
    // Filter empties per candidate, so a set-but-blank `KIRI_<ID>_API_KEY` does not shadow a real
    // vendor var.
    std::iter::once(generic.as_str())
        .chain(vendor.iter().copied())
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn generic_env_key(profile: &ProviderProfile) -> String {
    format!(
        "KIRI_{}_API_KEY",
        profile.id.to_ascii_uppercase().replace('-', "_")
    )
}

fn env_hint(profile: &ProviderProfile) -> String {
    match profile.kind {
        ProviderKind::Nvidia => "NVIDIA_API_KEY".into(),
        ProviderKind::Openai => "OPENAI_API_KEY".into(),
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY".into(),
        ProviderKind::OpenAiCompatible | ProviderKind::Custom => generic_env_key(profile),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_credential;
    use crate::modules::provider::application::secret_store::SecretStore;
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::provider::{
        AuthMethod, Credential, ProviderKind, ProviderProfile, Secret,
    };

    /// A `SecretStore` double returning a fixed stored credential (or none).
    struct FakeStore(Option<Credential>);
    impl SecretStore for FakeStore {
        fn get(&self, _id: &str) -> Result<Option<Credential>, AgentError> {
            Ok(self.0.clone())
        }
        fn set(&self, _id: &str, _credential: &Credential) -> Result<(), AgentError> {
            Ok(())
        }
        fn delete(&self, _id: &str) -> Result<(), AgentError> {
            Ok(())
        }
    }

    fn profile(id: &str, kind: ProviderKind, auth: AuthMethod, model: &str) -> ProviderProfile {
        ProviderProfile {
            id: id.to_string(),
            kind,
            base_url: "https://example.test/v1".to_string(),
            model: model.to_string(),
            models: vec![],
            auth,
        }
    }

    fn api_key() -> Credential {
        Credential::ApiKey {
            key: Secret::new("k"),
        }
    }

    #[test]
    fn resolve_credential_returns_the_stored_credential() {
        let store = FakeStore(Some(api_key()));
        let p = profile("nvidia", ProviderKind::Nvidia, AuthMethod::ApiKey, "m");
        match resolve_credential(&p, &store).unwrap() {
            Credential::ApiKey { key } => assert_eq!(key.expose(), "k"),
            other => panic!("expected api-key, got {other:?}"),
        }
    }

    #[test]
    fn resolve_credential_bails_when_absent_and_no_env() {
        // A Custom kind with a unique id: no vendor env var, and the generic KIRI_..._API_KEY is unset.
        let store = FakeStore(None);
        let p = profile(
            "unit-test-no-env",
            ProviderKind::Custom,
            AuthMethod::ApiKey,
            "m",
        );
        assert!(resolve_credential(&p, &store).is_err());
    }
}
