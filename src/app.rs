//! The composition root (ADR 0003). `wire` assembles the interactive TUI and `wire_sync` the headless
//! `kiri sync` route — the *only* places concrete adapters are chosen. For sync, both build the
//! git/shared-memory/work-tree adapters here and inject them as ports: `wire` packages them into a
//! [`SyncContext`] handed to the runtime (so a live `/sync` push constructs no adapter and recomputes no
//! path), and `wire_sync` runs the action directly. The shared store opens with a non-fatal init
//! (mirroring `build_memory`): `wire` injects a *factory* (`sync_memory_factory`) so the store opens
//! lazily on the first `/sync`, never birthing a `shared.db` for a memory-off session that never syncs;
//! `wire_sync` opens it eagerly since it is *running* sync (ADR 0015).

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

/// The composition root: build the sandbox, the provider adapter, the tool registry and the agent loop
/// from resolved settings, then assemble the full-screen TUI. This is the one place adapters are chosen.
/// The TUI requires an interactive terminal; a non-TTY stdout (piped output, CI) fails fast.
pub async fn wire(settings: Settings) -> Result<Tui> {
    if !std::io::stdout().is_terminal() {
        bail!("Kiri requires an interactive terminal (stdout is not a TTY)");
    }
    // Collect the non-fatal wire-time degradations (memory/session/embeddings/provider unavailable) so the
    // runtime can surface them in-transcript at boot, instead of `eprintln!` the alternate-screen TUI hides.
    let mut boot_notices: Vec<BootNotice> = Vec::new();
    // A timed HTTP client, built up front so both the chat provider and the (optional) embeddings adapter
    // share it.
    let client = build_http_client(settings.connect_timeout, settings.read_timeout)
        .context("failed to build the HTTP client")?;
    let secrets = default_secret_store(settings.credentials_file.clone());
    // The workspace key both session persistence and project memory are keyed by — derived once here and
    // threaded into `build_memory`/`TuiParams`. canonicalize fails only for a missing/permission-denied
    // path; the literal path is a safe fallback for project-id keying (a stable per-workspace key, not a
    // security boundary).
    let canonical_path = settings
        .path
        .canonicalize()
        .unwrap_or_else(|_| settings.path.clone());
    let project_id = project_id_from_path(&canonical_path);
    // Optional embeddings adapter for semantic recall; None (and a boot notice) on any misconfiguration,
    // so recall degrades to keyword rather than failing the boot.
    let embedder = build_embedder(&settings, &client, secrets.as_ref(), &mut boot_notices);
    // Memory & docs: a degraded store (init failure) is surfaced and left inert, never fatal.
    let (memory_tools, memory_digest, memory) =
        build_memory(&settings, embedder, project_id.clone(), &mut boot_notices).await?;
    // Session persistence shares the same degrade-never-abort contract as memory.
    let session_store = build_session(&settings, &mut boot_notices).await?;
    // Extensions (ADR 0021): rules + commands from the global and project layers. Auxiliary like memory
    // and session — a load failure degrades to an empty catalog rather than aborting the boot.
    let extensions = build_extensions(&settings, &mut boot_notices).await;
    // Shared by hook dispatch and MCP-server connection: the TOFU trust store gating both active
    // capability types (ADR 0021). One file backs every workspace/kind, so approvals are scoped by
    // `project_id` (reusing the same per-workspace key `build_memory`/sessions use) and by capability
    // kind (`is_approved`/`approve`'s `kind` argument) — otherwise a hook and an MCP server rendering the
    // same content string, or the same id+content reused across two projects, would share one approval.
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
    // The composition root owns cross-module wiring: build the sensitive matcher here and inject it into
    // the sandbox, so `Settings`/`config` no longer reaches into the `tools` adapter for it.
    let sensitive = load_sensitive_matcher()?;
    // Render the system prompt's tool/limit/sensitive facts from live single sources — the active
    // matcher's globs, the enforced run_command limits, and the checkpoint budget — before `sensitive`
    // moves into the sandbox, so an override is reflected and the prompt cannot lie about what the
    // harness blocks/enforces (SEC-06).
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
    // Resolve the active provider profile and its credential (the 0600 credentials file),
    // then select the adapter. This is the one place adapters are chosen.
    let profile = settings.active_profile()?.clone();
    let credential = resolve_credential(&profile, secrets.as_ref(), &mut boot_notices)?;
    // A keyless active provider whose id once held a key (migrated api-key -> none by hand-edit) leaves a
    // stale secret in the store; clear it best-effort so no orphaned credential lingers. A missing-key
    // delete is a harmless no-op.
    if profile.auth == AuthMethod::None {
        let _ = secrets.delete(&profile.id);
    }
    // Pick the initial adapter without ever aborting the boot (see `select_initial_provider`). The
    // client/credential are kept so the runtime's `ProviderSwap` can rebuild on a live `/effort` change
    // without a store round-trip.
    let (provider, needs_onboarding) =
        select_initial_provider(&client, &profile, &credential, &settings, &mut boot_notices);
    // The file tools plus the plan-mode control tool. `present_plan` is advertised only in plan mode
    // (it carries `plan_only`); the registry's `schemas()` withholds it everywhere else.
    let mut tools = default_fs_tools(settings.plan_allow.clone(), settings.require_confinement);
    tools.push(Arc::new(PresentPlan));
    tools.extend(memory_tools);
    tools.extend(default_extension_tools(Arc::new(extensions.skills.clone())));
    tools.extend(build_mcp_tools(&extensions, &trust_store, &mut boot_notices).await);
    // ADR 0029: the `task` tool dispatches a loaded agent profile as a nested, read-only subagent. Its
    // child pool is every tool assembled so far — never `task` itself, the structural depth-1 cap — so
    // it is built last, cloning the pool before it moves into the registry. Skipped entirely when no
    // agent profile is loaded, so a fresh install never advertises a dead tool.
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

    // The session's system prompt is the rendered base plus, when present, a digest of recalled memory.
    let system_prompt = if memory_digest.is_empty() {
        base_system_prompt
    } else {
        format!("{base_system_prompt}\n\n{memory_digest}")
    };

    // Build the sync ports here (the single composition root) and inject them, so a live `/sync` push
    // constructs no adapter and recomputes no path. The adapter choice lives only here. Built before the
    // provider swap consumes `settings.providers`/`active_provider`.
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
    );
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
    // ADR 0021 hook dispatch: the catalog, the sanctioned shell-exec adapter, and the TOFU trust store.
    // Built here (the composition root) and threaded through TuiParams to the SessionStart/SessionEnd/
    // TurnEnd firing points in `tui::infrastructure::runtime`. The catalog Arc is built last — every
    // other read of `extensions` above happens first.
    let hook_context = HookContext {
        catalog: Arc::new(extensions),
        runner: Arc::new(ShellHookRunner),
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

/// Bound on the number of tools registered from a single MCP server. `McpToolProxy` leaks each qualified
/// name (`Box::leak`, once per tool, never freed) — an approved-but-compromised server returning an
/// unbounded tool list must not leak unbounded memory or bloat the schema payload sent to the provider.
const MAX_MCP_TOOLS_PER_SERVER: usize = 200;

/// Build the shared HTTP client for provider and embeddings traffic. `read_timeout` bounds a stalled
/// stream without killing a long but active one. Redirects are disabled: every provider request carries a
/// credential header (Authorization/x-api-key), and reqwest's default redirect policy would replay that
/// header to whatever host a 3xx response names — a malicious or compromised provider endpoint could
/// exfiltrate the key to an attacker-controlled host this way (issue #24). No legitimate provider API
/// requires following a redirect.
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

/// Connect to every gate-approved MCP server (ADR 0021) and register its discovered tools. Global
/// servers always connect; a project server needs the trust gate to currently approve its exact
/// `(command, args)` (`/approve-mcp <id>`). A pending server, a spawn/handshake failure, or a discovery
/// failure all surface as a boot notice and are skipped — auxiliary, like every other extension type,
/// never fatal.
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
                // Fail closed on a trust-store read error (corrupt/unreadable file): treat as
                // not-yet-approved rather than propagating — a storage hiccup must never silently grant
                // a network-capable active capability. A retried `/approve-mcp` surfaces the same read
                // error directly (it calls `approve`, which reads-then-writes), so the failure isn't silent.
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

/// Wire the extensions framework (ADR 0021): rules, commands, agents, skills, hooks, and MCP servers,
/// discovered from `~/.kiri/{...}` (global, trusted) and `<workspace>/.kiri/{...}` (project), with the
/// binary-shipped defaults (ADR 0028) folded in as a third, lowest-precedence layer per resource type —
/// so a fresh install is never empty, and any user file overrides a default of the same id. Auxiliary
/// like memory/session — a load failure degrades to an empty catalog (surfaced as a boot notice) rather
/// than aborting the boot.
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

/// The headless `kiri sync …` route, wired through the single composition root: harden the harness home,
/// build the sync ports over the paths single-sourced from `Settings`, run the action, and print the
/// one-line summary. Never needs a terminal, so it works over SSH and in scripts.
pub async fn wire_sync(settings: &Settings, action: SyncAction) -> Result<()> {
    // Defense-in-depth: harden the harness home (0700) so `wire_sync` is self-contained even when called
    // directly (e.g. in tests). On the normal path `main.rs` resolves `Settings` first, which already
    // hardens `~/.kiri`, so this is a redundant-but-cheap guarantee rather than the sole protection.
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

/// Open the shared store for sync at `db`, mirroring `build_memory`'s non-fatal init for real: a failed
/// open or init degrades to an inert in-memory store rather than aborting (sync then degrades, never
/// crashes). Crucially, `init()` is called ONLY on the successfully opened on-disk store — the in-memory
/// fallback is left un-init'd, so it honestly reports `is_available() == false` (issue #33; previously
/// this called `init()` unconditionally on the fallback too, making a degraded store report available and
/// letting `kiri sync push` publish an empty `memory.ndjson` over the remote snapshot). A second SQLite
/// handle to the same file the memory tools use — safe, exactly what the prior on-demand open did
/// transiently. Returns the store plus an optional degraded-mode warning for the caller to surface on its
/// own channel — never swallowed, never printed from here, so it can't corrupt the live TUI on the lazy
/// `/sync` path.
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

/// Build the on-demand sync-store factory (the adapter choice stays here, the composition root). Capturing
/// only the path, it opens nothing until the first `/sync`, so a memory-off session that never syncs
/// births no `shared.db`.
fn sync_memory_factory(db: PathBuf) -> SharedMemoryFactory {
    Arc::new(move || {
        let db = db.clone();
        Box::pin(build_sync_memory_at(db))
    })
}

/// Wire the memory contexts (project file store + shared SQLite store) and the docs library, returning
/// the memory/docs tools and a start-of-session digest to inject into the system prompt. A store whose
/// `init` fails is surfaced as a boot notice and left inert (`is_available() == false`) rather than
/// aborting: memory is auxiliary, so the harness must still start. Returns no tools and an empty digest
/// when memory is disabled (`KIRI_MEMORY=off`). `project_id` is the workspace key derived once in `wire`.
async fn build_memory(
    settings: &Settings,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    project_id: String,
    notices: &mut Vec<BootNotice>,
) -> Result<(Vec<Arc<dyn Tool>>, String, Arc<dyn Memory>)> {
    if !settings.memory_enabled {
        return Ok((Vec::new(), String::new(), inert_memory_port(settings)?));
    }

    // Project memory: Markdown files under <workspace>/.kiri/memory.
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
        // Best-effort digest: continue with an empty list instead of aborting if the query fails.
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

    // Clone the port out before it is moved into the tools: the runtime also needs it to drive the
    // end-of-session distillation.
    let tools = default_memory_tools(memory.clone(), docs, project_id);
    let digest = render_digest(&project_entries, &shared_entries);
    Ok((tools, digest, memory))
}

/// Build the embeddings adapter from the `[embeddings]` config: it names an existing provider id whose
/// endpoint + credential to reuse, plus the model. Returns `None` (with a boot notice) on any
/// misconfiguration — an unknown provider, a missing credential, or an embeddings-less provider — so
/// semantic recall degrades to keyword rather than failing the boot.
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
        Ok(Some(credential)) => credential,
        Ok(None) => {
            notices.push(BootNotice::new(format!(
                "no credential for embeddings provider '{}'; semantic recall disabled",
                config.provider_id
            )));
            return None;
        }
        // Distinguish a genuine store fault from "not logged in", so a broken credential store
        // is diagnosable rather than silently reported as a missing credential.
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

/// Build an inert memory port (both scopes unavailable) for the memory-disabled boot, so the runtime can
/// hold a non-optional `Arc<dyn Memory>` whose every write is a graceful no-op.
fn inert_memory_port(settings: &Settings) -> Result<Arc<dyn Memory>> {
    let project = FileProjectMemory::new(settings.path.join(".kiri").join("memory"));
    let shared = SqliteSharedMemory::in_memory()?;
    // Neither store has init() called — both report is_available() = false (inert mode).
    Ok(Arc::new(LayeredMemory::new(project, shared)))
}

/// Wire the session store (SQLite at `~/.kiri/sessions.db`). Mirrors the memory contract: a store whose
/// `init` fails (or whose file cannot be opened) is surfaced as a boot notice and left inert
/// (`is_available() == false`) rather than aborting — conversation persistence is auxiliary. Returns an
/// inert in-memory store when memory is disabled (`KIRI_MEMORY=off`).
async fn build_session(
    settings: &Settings,
    notices: &mut Vec<BootNotice>,
) -> Result<Arc<dyn SessionStore>> {
    // The inert fallback mirrors `build_memory`'s `SqliteSharedMemory::in_memory()?`: its only failure
    // is an in-memory SQLite open, which means the process genuinely cannot run, so it propagates.
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

/// Pick the initial chat adapter without ever aborting the boot: with a usable credential AND a non-blank
/// model, build the real adapter; otherwise fall back to the null provider and raise onboarding. This
/// neutralizes every boot-crash path — no credential, credential-present-but-blank-model, and a
/// misconfigured profile `build_provider` rejects (a hand-edited/synced vendor set to auth = "none", or an
/// auth value this build does not recognize). Returns `(adapter, needs_onboarding)`.
fn select_initial_provider(
    client: &reqwest::Client,
    profile: &ProviderProfile,
    credential: &Option<Credential>,
    settings: &Settings,
    notices: &mut Vec<BootNotice>,
) -> (Arc<dyn CompletionProvider>, bool) {
    // The null-provider onboarding fallback, bound in one place (ROOT-08) so the two onboarding exits
    // below cannot drift.
    let onboarding =
        || -> (Arc<dyn CompletionProvider>, bool) { (Arc::new(UnconfiguredProvider::new()), true) };
    // A usable credential AND a non-blank model are both required to build the real adapter; anything
    // else (no credential, or a blank model) routes to onboarding rather than crashing.
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

/// Resolve the active provider's credential: the stored one if present, else a one-time import from a
/// legacy env var (migration aid / CI escape hatch) for API-key providers, else `None` — the signal that
/// this is a first run with nothing configured, which the caller routes to onboarding (never a fatal
/// abort). A genuine store error (a broken credentials file, distinct from "not logged in") still propagates.
/// Never logs the secret.
fn resolve_credential(
    profile: &ProviderProfile,
    secrets: &dyn SecretStore,
    notices: &mut Vec<BootNotice>,
) -> Result<Option<Credential>> {
    // Delegate the policy to the single resolver; this adapter only maps it to the boot shape
    // (`Option<Credential>`, where `None` routes to onboarding) and reports an env-import persist outcome.
    // The outcome is a `BootNotice` (not `eprintln!`) so it survives into the transcript rather than
    // flashing behind the alternate-screen TUI — both the SEC-07 persistence disclosure and the
    // persist-failure degradation must reach the user.
    match resolve_credential_policy(profile, secrets)? {
        CredentialResolution::Keyless => Ok(Some(Credential::None)),
        CredentialResolution::Stored(credential) => Ok(Some(credential)),
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
            Ok(Some(credential))
        }
        CredentialResolution::ImportedSessionOnly { credential } => {
            notices.push(BootNotice::new(format!(
                "imported the API key for provider '{}' from the environment for THIS SESSION ONLY \
                 (KIRI_NO_KEY_IMPORT is set); it was not saved to the credential store and does not \
                 persist across sessions.",
                profile.id
            )));
            Ok(Some(credential))
        }
        CredentialResolution::Absent => Ok(None),
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

    /// A loopback server that always replies with a 302 redirecting to a second loopback server, which
    /// would return 200 if actually reached. Hermetic (loopback only); the redirect target need not even
    /// be listening for the assertion below (a followed redirect would fail to connect and error, which
    /// still distinguishes "followed" from "did not follow").
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
        // Issue #24: a provider request carries a credential header; following a 3xx would replay it to
        // whatever host `Location` names. Asserting the client surfaces the 302 itself (not a response
        // from the redirect target, which reqwest's default policy would have followed to) proves the
        // policy is disabled.
        let response = serve_redirect_once().await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FOUND,
            "the client must return the 302 as-is, never follow it to the Location host"
        );
    }

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
            Some(Credential::ApiKey { key }) => assert_eq!(key.expose(), "k"),
            other => panic!("expected a stored api-key, got {other:?}"),
        }
    }

    #[test]
    fn resolve_credential_returns_none_when_absent_and_no_env() {
        // A Custom kind with a unique id: no vendor env var, and the generic KIRI_..._API_KEY is unset.
        // First run with nothing configured resolves to None (onboarding), never an abort.
        let store = FakeStore(None);
        let p = profile(
            "unit-test-no-env",
            ProviderKind::Custom,
            AuthMethod::ApiKey,
            "m",
        );
        assert!(
            resolve_credential(&p, &store, &mut Vec::new())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn resolve_credential_yields_none_credential_for_a_keyless_profile() {
        // auth = "none" short-circuits to Credential::None and must ignore any stale stored key — the
        // early return precedes the store lookup, so a leftover key from a prior keyed config is unused.
        let store = FakeStore(Some(api_key()));
        let p = profile(
            "lmstudio",
            ProviderKind::OpenAiCompatible,
            AuthMethod::None,
            "gemma",
        );
        match resolve_credential(&p, &store, &mut Vec::new()).unwrap() {
            Some(Credential::None) => {}
            other => panic!("expected Credential::None, got {other:?}"),
        }
    }

    // After Step 2, the sensitive matcher is built in `wire` (via `load_sensitive_matcher`) and injected
    // into the sandbox instead of carried on `Settings`. Lock that this build path still produces a
    // guarding sandbox — a sandbox wired from it must refuse a `.env` write — so the relocation did not
    // silently drop the secrets guard.
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

    /// A `Settings` whose harness-data paths all descend from `global_dir` (`Settings::resolve` itself is
    /// not unit-driven because it touches the real `$HOME`). The `shared_memory_db` uses a distinctive
    /// name a real resolve would never produce, so the `wire_sync` test proves it opened *that* path
    /// rather than a recomputed default. Lets `wire_sync` be driven against a temp dir.
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
