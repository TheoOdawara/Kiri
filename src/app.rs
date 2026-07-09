//! The composition root (ADR 0003): `wire` (TUI) and `wire_sync` (headless `kiri sync`) are the *only*
//! places concrete adapters are chosen. Both inject the sync ports rather than constructing them later, so
//! a live `/sync` builds no adapter and recomputes no path. `wire` injects a *factory* so the shared store
//! opens lazily on the first `/sync`, never birthing a `shared.db` for a session that never syncs;
//! `wire_sync` opens it eagerly, since it is *running* sync (ADR 0015).

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::modules::agent::application::agent_loop::AgentLoop;
use crate::modules::agent::infrastructure::task_tool::TaskTool;
use crate::modules::extensions::application::{ExtensionCatalog, ExtensionsLoader};
use crate::modules::extensions::domain::gate::{self, GateState, content_hash};
use crate::modules::extensions::domain::resource::McpServer;
use crate::modules::extensions::domain::scope::Layer;
use crate::modules::extensions::infrastructure::file_loader::FileExtensionsLoader;
use crate::modules::extensions::infrastructure::tools::default_extension_tools;
use crate::modules::extensions::infrastructure::trust_store::ExtensionsTrustStore;
use crate::modules::hooks::infrastructure::shell::ShellHookRunner;
use crate::modules::mcp::application::mcp_connection::McpConnection;
use crate::modules::mcp::infrastructure::rmcp_client::RmcpConnection;
use crate::modules::mcp::infrastructure::tool_proxy::McpToolProxy;
use crate::modules::memory::application::digest::{
    DIGEST_PROJECT_CAP, DIGEST_SHARED_CAP, render_digest,
};
use crate::modules::memory::application::memory_port::{LayeredMemory, Memory};
use crate::modules::memory::application::project_memory::ProjectMemory;
use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::domain::project_id::project_id_from_path;
use crate::modules::memory::infrastructure::docs_library::DocsLibrary;
use crate::modules::memory::infrastructure::file_project_memory::FileProjectMemory;
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::modules::memory::infrastructure::tools::default_memory_tools;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::application::embedding_provider::EmbeddingProvider;
use crate::modules::provider::application::secret_store::SecretStore;
use crate::modules::provider::infrastructure::factory::{
    CredentialResolution, build_embedding_provider, build_provider,
    resolve_credential as resolve_credential_policy,
};
use crate::modules::provider::infrastructure::secrets::default_secret_store;
use crate::modules::provider::infrastructure::unconfigured::UnconfiguredProvider;
use crate::modules::session::application::session_store::SessionStore;
use crate::modules::session::infrastructure::sqlite_session_store::SqliteSessionStore;
use crate::modules::sync::application::sync_service::SyncService;
use crate::modules::sync::infrastructure::fs_work_tree::FsSyncWorkTree;
use crate::modules::sync::infrastructure::git_cli::GitCli;
use crate::modules::sync::infrastructure::memory_ndjson::NdjsonMemoryExchange;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::application::tool::Tool;
use crate::modules::tools::infrastructure::args::RUN_COMMAND_DEFAULT_TIMEOUT_MS;
use crate::modules::tools::infrastructure::confine;
use crate::modules::tools::infrastructure::control::present_plan::PresentPlan;
use crate::modules::tools::infrastructure::exec::EXEC_MAX_BYTES;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::FsSandbox;
use crate::modules::tools::infrastructure::sensitive::load_sensitive_matcher;
use crate::modules::tui::domain::command_menu::CustomCommandEntry;
use crate::modules::tui::infrastructure::runtime::{
    BootNotice, HookContext, ProviderSwap, SharedMemoryFactory, SyncContext, Tui, TuiParams,
};
use crate::shared::infra::config::{
    PromptExtensions, Settings, SyncAction, ensure_private_dir, render_system_prompt,
};
use crate::shared::kernel::error::AgentResult;
use crate::shared::kernel::provider::{AuthMethod, Credential, ProviderProfile};

/// The one place adapters are chosen. The TUI requires an interactive terminal; a non-TTY stdout (piped
/// output, CI) fails fast.
pub async fn wire(settings: Settings) -> Result<Tui> {
    if !std::io::stdout().is_terminal() {
        bail!("Kiri requires an interactive terminal (stdout is not a TTY)");
    }
    // Collected rather than `eprintln!`d, which the alternate-screen TUI would hide: the runtime surfaces
    // these in-transcript at boot.
    let mut boot_notices: Vec<BootNotice> = Vec::new();
    let client = build_http_client(settings.connect_timeout, settings.read_timeout)
        .context("failed to build the HTTP client")?;
    let secrets = default_secret_store(settings.credentials_file.clone());
    // canonicalize fails only on a missing/permission-denied path, and the literal path is a safe fallback:
    // this keys per-workspace state, it is not a security boundary.
    let canonical_path = settings
        .path
        .canonicalize()
        .unwrap_or_else(|_| settings.path.clone());
    let project_id = project_id_from_path(&canonical_path);
    let embedder = build_embedder(&settings, &client, secrets.as_ref(), &mut boot_notices);
    let (memory_tools, memory_digest, memory) =
        build_memory(&settings, embedder, project_id.clone(), &mut boot_notices).await?;
    let session_store = build_session(&settings, &mut boot_notices).await?;
    let extensions = build_extensions(&settings, &mut boot_notices).await;
    // ADR 0021. One file backs every workspace and capability kind, so approvals are scoped by both:
    // otherwise a hook and an MCP server rendering the same content, or one id+content reused across two
    // projects, would share a single approval.
    let trust_store = Arc::new(ExtensionsTrustStore::new(
        settings.global_dir.join("extensions_trust.json"),
        project_id.clone(),
    ));
    let rules_text = extensions.render_rules();
    let skills_text = extensions.skills_index().unwrap_or_default();
    let agents_text = extensions.agents_index().unwrap_or_default();
    let hooks_display = extensions.hooks_display();
    let mcp_display = extensions.mcp_display();
    let confiner = confine::default_command_sandbox(settings.sandbox_enabled);
    // #112: honest boot notice when OS confinement is unavailable (Windows residual #90).
    if !confiner.supports_confinement() {
        let message = if settings.require_confinement {
            "OS command sandbox unavailable on this platform; KIRI_SANDBOX=require will refuse \
             run_command and hooks (path policy + confirmation still apply)."
                .to_string()
        } else {
            "OS command sandbox unavailable on this platform; run_command/hooks use path policy + \
             confirmation only (no OS jail)."
                .to_string()
        };
        boot_notices.push(BootNotice::new(message));
    }
    // Built here and injected, so `config` never reaches into the `tools` adapter for it.
    let sensitive = load_sensitive_matcher()?;
    // Render the prompt's tool/limit/sensitive facts from the live sources before `sensitive` moves into
    // the sandbox, so an override is reflected and the prompt cannot lie about what the harness blocks
    // (SEC-06).
    let instructions_display = settings.instructions_display();
    let base_system_prompt = render_system_prompt(
        &sensitive.globs(),
        RUN_COMMAND_DEFAULT_TIMEOUT_MS,
        EXEC_MAX_BYTES,
        settings.checkpoint_budget,
        PromptExtensions {
            rules: (!rules_text.is_empty()).then_some(rules_text.as_str()),
            skills: (!skills_text.is_empty()).then_some(skills_text.as_str()),
            agents: (!agents_text.is_empty()).then_some(agents_text.as_str()),
            instructions_global: settings.instructions_global.as_deref(),
            instructions_project: settings.instructions_project.as_deref(),
        },
    );
    let sandbox = FsSandbox::with_confinement(
        &settings.path,
        sensitive,
        confiner,
        settings.sandbox_network,
        settings.extra_ro.clone(),
        settings.extra_rw.clone(),
    )?;
    let profile = settings.active_profile()?.clone();
    let (credential, credential_session_only) =
        resolve_credential(&profile, secrets.as_ref(), &mut boot_notices)?;
    // Deliberately ignored: a provider hand-edited from api-key to none leaves a stale secret behind.
    // Clearing it is best-effort, and deleting a missing key is a harmless no-op.
    if profile.auth == AuthMethod::None {
        let _ = secrets.delete(&profile.id);
    }
    // The client and credential are kept so a live `/effort` change can rebuild the adapter through
    // `ProviderSwap` with no store round-trip.
    let (provider, needs_onboarding) =
        select_initial_provider(&client, &profile, &credential, &settings, &mut boot_notices);
    // `present_plan` carries `plan_only`, so the registry's `schemas()` withholds it outside plan mode.
    let mut tools = default_fs_tools(settings.plan_allow.clone(), settings.require_confinement);
    tools.push(Arc::new(PresentPlan));
    tools.extend(memory_tools);
    tools.extend(default_extension_tools(Arc::new(extensions.skills.clone())));
    tools.extend(build_mcp_tools(&extensions, &trust_store, &mut boot_notices).await);
    // ADR 0029. Built last because its child pool is every tool assembled so far — never `task` itself,
    // which is the structural depth-1 cap. Skipped when no agent profile is loaded, so a fresh install
    // never advertises a dead tool.
    let agents = Arc::new(extensions.agents.clone());
    if !agents.is_empty() {
        tools.push(Arc::new(TaskTool::new(
            provider.clone(),
            tools.clone(),
            agents,
            profile.model.clone(),
            settings.checkpoint_budget,
            settings.max_tool_calls,
        )));
    }
    let registry = ToolRegistry::new(tools);
    let agent_loop = AgentLoop::new(
        provider,
        registry,
        profile.model.clone(),
        settings.checkpoint_budget,
        settings.max_tool_calls,
    );

    let system_prompt = if memory_digest.is_empty() {
        base_system_prompt
    } else {
        format!("{base_system_prompt}\n\n{memory_digest}")
    };

    // Built before the provider swap consumes `settings.providers`/`active_provider`.
    let sync_context = SyncContext::new(
        Arc::new(GitCli),
        sync_memory_factory(settings.shared_memory_db.clone()),
        Arc::new(FsSyncWorkTree),
        settings.global_dir.clone(),
        settings.config_path.clone(),
    );
    let provider_swap = ProviderSwap::new(
        client,
        secrets,
        settings.providers,
        settings.active_provider,
        credential,
        settings.thinking,
        settings.effort,
    )
    .with_session_only_credential(credential_session_only);
    let rules_display = extensions.rules_display();
    let commands_display = extensions.commands_display();
    let agents_display = extensions.agents_display();
    let skills_display = extensions.skills_display();
    let custom_command_bodies = extensions.command_bodies();
    let mut custom_commands: Vec<CustomCommandEntry> = extensions
        .commands
        .values()
        .map(|command| CustomCommandEntry {
            name: command.name.clone(),
            blurb: command.description.clone(),
        })
        .collect();
    custom_commands.sort_by(|a, b| a.name.cmp(&b.name));
    // ADR 0021 hook dispatch, threaded through `TuiParams` to the firing points in the runtime. The
    // catalog `Arc` is built last: every other read of `extensions` above happens first.
    let hook_context = HookContext {
        catalog: Arc::new(extensions),
        runner: Arc::new(ShellHookRunner::new(settings.require_confinement)),
        trust: trust_store,
    };
    Ok(Tui::new(TuiParams {
        agent_loop,
        sandbox,
        system_prompt,
        seed: settings.seed,
        provider_swap,
        config_path: settings.config_path,
        sync_context,
        needs_onboarding,
        session_store,
        memory,
        project_id,
        boot_notices,
        instructions_display,
        rules_display,
        custom_commands,
        custom_command_bodies,
        commands_display,
        agents_display,
        skills_display,
        hooks_display,
        mcp_display,
        hooks: hook_context,
    }))
}

/// `McpToolProxy` leaks each qualified name (`Box::leak`, never freed), so an approved-but-compromised
/// server returning an unbounded tool list must not leak unbounded memory or bloat the schema payload.
const MAX_MCP_TOOLS_PER_SERVER: usize = 200;

/// Redirects are disabled: every provider request carries a credential header, and reqwest's default
/// policy would replay it to whatever host a 3xx names, letting a compromised endpoint exfiltrate the key
/// (issue #24). No legitimate provider API requires following a redirect.
fn build_http_client(
    connect_timeout: Duration,
    read_timeout: Duration,
) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// ADR 0021. Global servers always connect; a project server needs the trust gate to approve its exact
/// `(command, args)`. A pending server, or any spawn/handshake/discovery failure, becomes a boot notice
/// and is skipped — auxiliary, never fatal.
async fn build_mcp_tools(
    extensions: &ExtensionCatalog,
    trust: &ExtensionsTrustStore,
    notices: &mut Vec<BootNotice>,
) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    let mut servers: Vec<&McpServer> = extensions.mcp_servers.values().collect();
    servers.sort_by(|a, b| a.id.cmp(&b.id));
    for server in servers {
        let approved = match server.layer {
            Layer::Global | Layer::Bundled => true,
            Layer::Project => {
                let hash = content_hash(&server.hash_key());
                // Fail closed on a trust-store read error: a storage hiccup must never silently grant a
                // network-capable capability. A retried `/approve-mcp` surfaces the same read error
                // directly, so the failure is not swallowed.
                let previously_approved =
                    trust.is_approved("mcp", &server.id, &hash).unwrap_or(false);
                gate::resolve(server.layer, previously_approved) == GateState::Approved
            }
        };
        if !approved {
            notices.push(BootNotice::new(format!(
                "MCP server '{}' is pending approval (project layer) — it will run as an unrestricted, \
                 network-capable subprocess once approved; run /approve-mcp {} to enable it",
                server.id, server.id
            )));
            continue;
        }
        let connection = match RmcpConnection::connect(&server.command, &server.args).await {
            Ok(connection) => Arc::new(connection) as Arc<dyn McpConnection>,
            Err(error) => {
                notices.push(BootNotice::new(format!(
                    "MCP server '{}' unavailable ({error}); continuing without its tools",
                    server.id
                )));
                continue;
            }
        };
        match connection.list_tools().await {
            Ok(specs) => {
                let discovered = specs.len();
                for spec in specs.into_iter().take(MAX_MCP_TOOLS_PER_SERVER) {
                    tools.push(Arc::new(McpToolProxy::new(
                        &server.id,
                        spec,
                        connection.clone(),
                    )));
                }
                if discovered > MAX_MCP_TOOLS_PER_SERVER {
                    notices.push(BootNotice::new(format!(
                        "MCP server '{}' advertised {discovered} tools; only the first \
                         {MAX_MCP_TOOLS_PER_SERVER} were registered",
                        server.id
                    )));
                }
            }
            Err(error) => notices.push(BootNotice::new(format!(
                "MCP server '{}' connected but tool discovery failed ({error}); continuing without its \
                 tools",
                server.id
            ))),
        }
    }
    tools
}

/// ADR 0021/0028. The binary-shipped defaults fold in as a third, lowest-precedence layer, so a fresh
/// install is never empty and a user file always overrides a default of the same id. A load failure
/// degrades to an empty catalog rather than aborting the boot.
async fn build_extensions(settings: &Settings, notices: &mut Vec<BootNotice>) -> ExtensionCatalog {
    let loader = FileExtensionsLoader::new(settings.global_dir.clone(), &settings.path);
    match loader.load().await {
        Ok(catalog) => catalog,
        Err(error) => {
            notices.push(BootNotice::new(format!(
                "extensions unavailable ({error}); continuing without rules/commands"
            )));
            ExtensionCatalog::default()
        }
    }
}

/// The headless `kiri sync …` route. Never needs a terminal, so it works over SSH and in scripts.
pub async fn wire_sync(settings: &Settings, action: SyncAction) -> Result<()> {
    // Defense in depth: `main.rs` resolves `Settings` first, which already hardens `~/.kiri`. Repeating it
    // is cheap and makes `wire_sync` self-contained when called directly, as tests do.
    ensure_private_dir(&settings.global_dir)?;
    let (memory, warning) = build_sync_memory_at(settings.shared_memory_db.clone()).await?;
    if let Some(reason) = warning {
        eprintln!("kiri: {reason}");
    }
    let git = GitCli;
    let work_tree = FsSyncWorkTree;
    let exchange = NdjsonMemoryExchange::new(memory.as_ref());
    let service = SyncService::new(
        &git,
        settings.global_dir.clone(),
        settings.config_path.clone(),
        &exchange,
        &work_tree,
    );
    let summary = match action {
        SyncAction::Init { url } => service.init(&url).await,
        SyncAction::Push => service.push().await,
        SyncAction::Pull { force } => service.pull(force).await,
        SyncAction::Status => service.status().await,
    }?;
    println!("kiri sync: {summary}");
    Ok(())
}

/// A failed open or init degrades to an inert in-memory store rather than aborting. `init()` runs ONLY on
/// the on-disk store: leaving the fallback un-init'd is what makes it report `is_available() == false`, so
/// `kiri sync push` cannot publish an empty `memory.ndjson` over the remote snapshot. The warning is
/// returned, not printed, so a lazy `/sync` cannot corrupt the live TUI. Opening a second SQLite handle to
/// the memory tools' file is safe.
async fn build_sync_memory_at(db: PathBuf) -> AgentResult<(Arc<dyn SharedMemory>, Option<String>)> {
    let (memory, warning) = match SqliteSharedMemory::new(db) {
        Ok(memory) => match memory.init().await {
            Ok(()) => (memory, None),
            Err(error) => (
                memory,
                Some(format!(
                    "shared memory for sync init failed ({error}); continuing with an inert store"
                )),
            ),
        },
        Err(error) => (
            SqliteSharedMemory::in_memory()?,
            Some(format!(
                "shared memory for sync unavailable ({error}); continuing with an inert store"
            )),
        ),
    };
    Ok((Arc::new(memory), warning))
}

/// Capturing only the path, it opens nothing until the first `/sync`, so a memory-off session that never
/// syncs births no `shared.db`.
fn sync_memory_factory(db: PathBuf) -> SharedMemoryFactory {
    Arc::new(move || {
        let db = db.clone();
        Box::pin(build_sync_memory_at(db))
    })
}

/// Returns the memory/docs tools plus a start-of-session digest for the system prompt. Memory is
/// auxiliary: a store whose `init` fails becomes a boot notice and is left inert, never aborting the boot.
/// With `KIRI_MEMORY=off`, no tools and an empty digest.
async fn build_memory(
    settings: &Settings,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    project_id: String,
    notices: &mut Vec<BootNotice>,
) -> Result<(Vec<Arc<dyn Tool>>, String, Arc<dyn Memory>)> {
    if !settings.memory_enabled {
        return Ok((Vec::new(), String::new(), inert_memory_port(settings)?));
    }

    let project_memory = FileProjectMemory::new(settings.path.join(".kiri").join("memory"));
    let project_ok = match project_memory.init().await {
        Ok(()) => true,
        Err(error) => {
            notices.push(BootNotice::new(format!(
                "project memory unavailable ({error}); continuing without it"
            )));
            false
        }
    };
    let project_entries = if project_ok {
        // Best-effort: a listing failure must not block the session, so the digest degrades to empty.
        project_memory
            .list(0, DIGEST_PROJECT_CAP)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Falls back to an inert in-memory database when the on-disk one cannot be opened.
    let (shared_memory, shared_ok) =
        match SqliteSharedMemory::new(settings.shared_memory_db.clone()) {
            Ok(memory) => match memory.init().await {
                Ok(()) => (memory, true),
                Err(error) => {
                    notices.push(BootNotice::new(format!(
                        "shared memory unavailable ({error}); continuing without it"
                    )));
                    (memory, false)
                }
            },
            Err(error) => {
                notices.push(BootNotice::new(format!(
                    "shared memory unavailable ({error}); continuing without it"
                )));
                (SqliteSharedMemory::in_memory()?, false)
            }
        };
    let shared_entries = if shared_ok {
        shared_memory
            .list(0, DIGEST_SHARED_CAP)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let port = LayeredMemory::new(project_memory, shared_memory);
    let memory: Arc<dyn Memory> = match embedder {
        Some(embedder) => Arc::new(port.with_embedder(embedder)),
        None => Arc::new(port),
    };
    let docs = Arc::new(DocsLibrary::new(settings.docs_path.clone()));

    // Cloned before it moves into the tools: the runtime needs it to drive end-of-session distillation.
    let tools = default_memory_tools(memory.clone(), docs, project_id);
    let digest = render_digest(&project_entries, &shared_entries);
    Ok((tools, digest, memory))
}

/// `None` (with a boot notice) on any misconfiguration, so semantic recall degrades to keyword rather than
/// failing the boot.
fn build_embedder(
    settings: &Settings,
    client: &reqwest::Client,
    secrets: &dyn SecretStore,
    notices: &mut Vec<BootNotice>,
) -> Option<Arc<dyn EmbeddingProvider>> {
    let config = settings.embeddings.as_ref()?;
    let Some(profile) = settings
        .providers
        .iter()
        .find(|p| p.id == config.provider_id)
    else {
        notices.push(BootNotice::new(format!(
            "embeddings provider '{}' is not in the catalog; semantic recall disabled",
            config.provider_id
        )));
        return None;
    };
    let credential = match resolve_credential(profile, secrets, notices) {
        Ok((Some(credential), _)) => credential,
        Ok((None, _)) => {
            notices.push(BootNotice::new(format!(
                "no credential for embeddings provider '{}'; semantic recall disabled",
                config.provider_id
            )));
            return None;
        }
        // Distinguished from "not logged in", so a broken credential store is diagnosable rather than
        // silently reported as a missing credential.
        Err(error) => {
            notices.push(BootNotice::new(format!(
                "embeddings credential store error for '{}' ({error}); semantic recall disabled",
                config.provider_id
            )));
            return None;
        }
    };
    match build_embedding_provider(client.clone(), profile, credential, config.model.clone()) {
        Ok(embedder) => Some(embedder),
        Err(error) => {
            notices.push(BootNotice::new(format!(
                "embeddings disabled ({error}); semantic recall falls back to keyword"
            )));
            None
        }
    }
}

/// For the memory-disabled boot, so the runtime can hold a non-optional `Arc<dyn Memory>` whose every
/// write is a graceful no-op.
fn inert_memory_port(settings: &Settings) -> Result<Arc<dyn Memory>> {
    let project = FileProjectMemory::new(settings.path.join(".kiri").join("memory"));
    let shared = SqliteSharedMemory::in_memory()?;
    // Neither store has init() called — both report is_available() = false (inert mode).
    Ok(Arc::new(LayeredMemory::new(project, shared)))
}

/// Mirrors [`build_memory`]'s contract: conversation persistence is auxiliary, so a store that fails to
/// open or init is left inert rather than aborting.
async fn build_session(
    settings: &Settings,
    notices: &mut Vec<BootNotice>,
) -> Result<Arc<dyn SessionStore>> {
    // The fallback's only failure is an in-memory SQLite open, which means the process genuinely cannot
    // run — so it propagates instead of degrading further.
    let inert = || -> Result<Arc<dyn SessionStore>> {
        Ok(Arc::new(SqliteSessionStore::in_memory_inert()?))
    };
    if !settings.memory_enabled {
        return inert();
    }
    match SqliteSessionStore::new(settings.sessions_db.clone()) {
        Ok(store) => match store.init().await {
            Ok(()) => Ok(Arc::new(store)),
            Err(error) => {
                notices.push(BootNotice::new(format!(
                    "session store unavailable ({error}); continuing without it"
                )));
                inert()
            }
        },
        Err(error) => {
            notices.push(BootNotice::new(format!(
                "session store unavailable ({error}); continuing without it"
            )));
            inert()
        }
    }
}

/// Never aborts the boot. Every failure path — no credential, a blank model, or a misconfigured profile
/// `build_provider` rejects — falls back to the null provider and raises onboarding.
fn select_initial_provider(
    client: &reqwest::Client,
    profile: &ProviderProfile,
    credential: &Option<Credential>,
    settings: &Settings,
    notices: &mut Vec<BootNotice>,
) -> (Arc<dyn CompletionProvider>, bool) {
    // Bound in one place (ROOT-08) so the two onboarding exits below cannot drift.
    let onboarding =
        || -> (Arc<dyn CompletionProvider>, bool) { (Arc::new(UnconfiguredProvider::new()), true) };
    let (Some(cred), true) = (credential, !profile.model.trim().is_empty()) else {
        return onboarding();
    };
    match build_provider(
        client.clone(),
        profile,
        cred.clone(),
        settings.thinking,
        settings.effort,
    ) {
        Ok(provider) => (provider, false),
        Err(error) => {
            notices.push(BootNotice::new(format!(
                "active provider '{}' could not be initialized ({error}); starting in onboarding",
                profile.id
            )));
            onboarding()
        }
    }
}

/// The stored credential, else a one-time env-var import, else `None` — a first run with nothing
/// configured, which the caller routes to onboarding rather than aborting. Second return is true when
/// the credential is SEC-07 session-only (`KIRI_NO_KEY_IMPORT`) and must not be written on blank-key
/// edit (#60). A genuine store error (a broken credentials file, as distinct from "not logged in") still
/// propagates. Never logs the secret.
fn resolve_credential(
    profile: &ProviderProfile,
    secrets: &dyn SecretStore,
    notices: &mut Vec<BootNotice>,
) -> Result<(Option<Credential>, bool)> {
    // The policy lives in the single resolver; this only maps it to the boot shape. Each outcome is a
    // `BootNotice`, not an `eprintln!`, so the SEC-07 persistence disclosure reaches the transcript instead
    // of flashing behind the alternate-screen TUI.
    match resolve_credential_policy(profile, secrets)? {
        CredentialResolution::Keyless => Ok((Some(Credential::None), false)),
        CredentialResolution::Stored(credential) => Ok((Some(credential), false)),
        CredentialResolution::Imported {
            credential,
            persisted,
        } => {
            match persisted {
                Ok(()) => notices.push(BootNotice::new(format!(
                    "imported the API key for provider '{}' from the environment and saved it to the \
                     credential store (the ~/.kiri/credentials.json file); it now persists \
                     across sessions on this machine. To undo this, remove the stored credential (a \
                     /provider logout flow is planned) — unsetting the env var does NOT delete the saved \
                     copy.",
                    profile.id
                ))),
                Err(error) => notices.push(BootNotice::new(format!(
                    "could not persist the credential for '{}' ({error}); using it this session only",
                    profile.id
                ))),
            }
            Ok((Some(credential), false))
        }
        CredentialResolution::ImportedSessionOnly { credential } => {
            notices.push(BootNotice::new(format!(
                "imported the API key for provider '{}' from the environment for THIS SESSION ONLY \
                 (KIRI_NO_KEY_IMPORT is set); it was not saved to the credential store and does not \
                 persist across sessions.",
                profile.id
            )));
            Ok((Some(credential), true))
        }
        CredentialResolution::Absent => Ok((None, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Settings, build_http_client, build_sync_memory_at, resolve_credential, sync_memory_factory,
        wire_sync,
    };
    use crate::modules::provider::application::secret_store::SecretStore;
    use crate::shared::kernel::error::AgentError;
    use crate::shared::kernel::provider::{
        AuthMethod, Credential, ProviderKind, ProviderProfile, Secret,
    };
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Replies 302 pointing at a second loopback address that need not be listening: a followed redirect
    /// would fail to connect and error, which still distinguishes "followed" from "did not follow".
    async fn serve_redirect_once() -> reqwest::Response {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let body = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/exfiltrate\r\n\
                            Content-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.flush().await;
            }
        });
        let client = build_http_client(Duration::from_secs(5), Duration::from_secs(5)).unwrap();
        client
            .get(format!("http://{addr}/"))
            .header("authorization", "Bearer super-secret-key")
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn build_http_client_does_not_follow_a_redirect() {
        // Issue #24: following a 3xx would replay the credential header to whatever host `Location` names.
        // Surfacing the 302 itself, rather than a response from the redirect target, proves it is disabled.
        let response = serve_redirect_once().await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FOUND,
            "the client must return the 302 as-is, never follow it to the Location host"
        );
    }

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
            thinking: None,
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
        match resolve_credential(&p, &store, &mut Vec::new()).unwrap() {
            (Some(Credential::ApiKey { key }), false) => assert_eq!(key.expose(), "k"),
            other => panic!("expected a stored api-key, got {other:?}"),
        }
    }

    #[test]
    fn resolve_credential_returns_none_when_absent_and_no_env() {
        // A Custom kind with a unique id has no vendor env var, and the generic KIRI_..._API_KEY is unset.
        let store = FakeStore(None);
        let p = profile(
            "unit-test-no-env",
            ProviderKind::Custom,
            AuthMethod::ApiKey,
            "m",
        );
        let (cred, session_only) = resolve_credential(&p, &store, &mut Vec::new()).unwrap();
        assert!(cred.is_none());
        assert!(!session_only);
    }

    #[test]
    fn resolve_credential_yields_none_credential_for_a_keyless_profile() {
        // The early return precedes the store lookup, so a leftover key from a prior keyed config is unused.
        let store = FakeStore(Some(api_key()));
        let p = profile(
            "lmstudio",
            ProviderKind::OpenAiCompatible,
            AuthMethod::None,
            "gemma",
        );
        match resolve_credential(&p, &store, &mut Vec::new()).unwrap() {
            (Some(Credential::None), false) => {}
            other => panic!("expected Credential::None, got {other:?}"),
        }
    }

    // The matcher moved from `Settings` into `wire`. Lock that a sandbox wired from it still refuses a
    // `.env` write, so the relocation did not silently drop the secrets guard.
    #[test]
    fn wire_builds_sensitive_matcher() {
        use crate::modules::tools::application::sandbox::Sandbox;
        use crate::modules::tools::infrastructure::sandbox::FsSandbox;
        use crate::modules::tools::infrastructure::sensitive::load_sensitive_matcher;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let sensitive = load_sensitive_matcher().unwrap();
        let sandbox = FsSandbox::new(dir.path(), sensitive).unwrap();
        assert!(
            sandbox.resolve_create(".env").is_err(),
            "the matcher built in wire must still refuse a .env path"
        );
    }

    /// Lets `wire_sync` be driven against a temp dir, since `Settings::resolve` touches the real `$HOME`.
    /// `shared_memory_db` takes a name a real resolve would never produce, so the test below proves it
    /// opened *that* path rather than a recomputed default.
    fn settings_at(global_dir: std::path::PathBuf) -> Settings {
        use crate::shared::kernel::provider::Effort;
        use crate::shared::kernel::sandbox::NetworkPolicy;
        use std::time::Duration;

        Settings {
            path: global_dir.clone(),
            seed: None,
            checkpoint_budget: Duration::from_secs(1),
            max_tool_calls: 1,
            plan_allow: Arc::from(Vec::new()),
            sandbox_enabled: false,
            require_confinement: false,
            sandbox_network: NetworkPolicy::Deny,
            extra_ro: Arc::from(Vec::new()),
            extra_rw: Arc::from(Vec::new()),
            connect_timeout: Duration::from_secs(1),
            read_timeout: Duration::from_secs(1),
            thinking: false,
            memory_enabled: true,
            docs_path: global_dir.join("docs"),
            shared_memory_db: global_dir.join("sync-test-shared.db"),
            sessions_db: global_dir.join("sessions.db"),
            credentials_file: global_dir.join("credentials.json"),
            global_dir: global_dir.clone(),
            config_path: global_dir.join("config.toml"),
            providers: vec![],
            active_provider: String::new(),
            effort: Effort::High,
            embeddings: None,
            instructions_global: None,
            instructions_project: None,
            instruction_paths: vec![],
        }
    }

    #[test]
    fn settings_exposes_global_dir() {
        // The single-source contract every sync consumer relies on: the data paths descend from one
        // `global_dir`, never a re-derived `config_path.parent()`.
        let settings = settings_at(std::path::PathBuf::from("/kiri-test-home"));
        assert_eq!(
            settings.global_dir,
            std::path::PathBuf::from("/kiri-test-home")
        );
        assert!(settings.shared_memory_db.starts_with(&settings.global_dir));
        assert!(settings.sessions_db.starts_with(&settings.global_dir));
        assert_eq!(
            settings.config_path.parent(),
            Some(settings.global_dir.as_path())
        );
    }

    #[tokio::test]
    async fn wire_sync_uses_settings_shared_memory_db() {
        use crate::shared::infra::config::SyncAction;
        let dir = tempfile::TempDir::new().unwrap();
        let settings = settings_at(dir.path().to_path_buf());
        // Status on an uninitialized tree errors ("not initialized"): proves wire_sync derived the sync
        // dir from settings.global_dir and never reconstructed a path by hand (ROOT-01 stale-DB lock).
        let err = wire_sync(&settings, SyncAction::Status).await.unwrap_err();
        assert!(format!("{err:#}").contains("not initialized"), "{err:#}");
        // And it opened the DB at settings.shared_memory_db (creating it), not a recomputed location.
        assert!(
            settings.shared_memory_db.exists(),
            "wire_sync must open the Settings-provided shared.db"
        );
    }

    #[tokio::test]
    async fn sync_memory_factory_defers_the_open() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("lazy-shared.db");
        let factory = sync_memory_factory(db.clone());
        // Building the factory must touch the disk for nothing: no shared.db until the first /sync.
        assert!(
            !db.exists(),
            "building the factory must open nothing — a memory-off session that never syncs births no shared.db"
        );
        let (_memory, warning) = (factory)()
            .await
            .expect("the factory opens the store on demand");
        assert!(
            db.exists(),
            "invoking the factory opens the store, creating shared.db on demand"
        );
        assert!(
            warning.is_none(),
            "a clean open over a writable path yields no degraded-mode warning"
        );
    }

    #[tokio::test]
    async fn build_sync_memory_at_open_failure_leaves_the_fallback_inert() {
        // Issue #33: `build_sync_memory_at` must mirror `build_memory`'s honest degrade — `init()` is
        // called ONLY on a successfully opened on-disk store, never on the in-memory fallback. Force the
        // open to fail by placing a directory at the exact db path (SQLite cannot open a directory as a
        // database file), then assert the returned store reports unavailable rather than silently
        // reporting healthy.
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("shared.db");
        std::fs::create_dir_all(&db).unwrap();

        let (memory, warning) = build_sync_memory_at(db).await.unwrap();
        assert!(warning.is_some(), "an open failure must surface a warning");
        assert!(
            !memory.is_available(),
            "the in-memory fallback must stay inert (never init'd) on an open failure"
        );
    }
}
