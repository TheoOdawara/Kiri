use std::io::IsTerminal;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::modules::agent::application::agent_loop::AgentLoop;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::infrastructure::openai::provider::OpenAiProvider;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::infrastructure::confine;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tui::infrastructure::runtime::Tui;
use crate::shared::infra::config::Settings;

/// The composition root: build the sandbox, the provider adapter, the tool registry and the agent loop
/// from resolved settings, then assemble the full-screen TUI. This is the one place adapters are chosen.
/// The TUI requires an interactive terminal; a non-TTY stdout (piped output, CI) fails fast.
pub fn wire(settings: Settings) -> Result<Tui> {
    if !std::io::stdout().is_terminal() {
        bail!("Kiri requires an interactive terminal (stdout is not a TTY)");
    }
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
    let registry = ToolRegistry::new(default_fs_tools(
        settings.plan_blacklist.clone(),
        settings.net_allow.clone(),
        settings.require_confinement,
    ));
    let model = settings.model.clone();
    let agent_loop = AgentLoop::new(
        provider,
        registry,
        settings.model,
        settings.checkpoint_budget,
        settings.max_tool_calls,
    );

    Ok(Tui::new(
        agent_loop,
        sandbox,
        settings.system_prompt,
        settings.seed,
        model,
    ))
}
