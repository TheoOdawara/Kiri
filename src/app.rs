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
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::infrastructure::openai::provider::OpenAiProvider;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::application::tool::Tool;
use crate::modules::tools::infrastructure::confine;
use crate::modules::tools::infrastructure::control::present_plan::PresentPlan;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tui::infrastructure::runtime::Tui;
use crate::shared::infra::config::Settings;

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
    let provider: Arc<dyn CompletionProvider> = Arc::new(OpenAiProvider::new(
        client,
        settings.base_url,
        settings.api_key,
        settings.thinking,
    ));
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
    let model = settings.model.clone();
    let agent_loop = AgentLoop::new(
        provider,
        registry,
        settings.model,
        settings.checkpoint_budget,
        settings.max_tool_calls,
    );

    // The session's system prompt is the static base plus, when present, a digest of recalled memory.
    let system_prompt = if memory_digest.is_empty() {
        settings.system_prompt.to_string()
    } else {
        format!("{}\n\n{}", settings.system_prompt, memory_digest)
    };

    Ok(Tui::new(
        agent_loop,
        sandbox,
        system_prompt,
        settings.seed,
        model,
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
