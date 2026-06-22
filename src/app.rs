use std::io::IsTerminal;
use std::sync::Arc;

use anyhow::Result;

use crate::modules::agent::application::agent_loop::AgentLoop;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::infrastructure::openai::provider::OpenAiProvider;
use crate::modules::repl::infrastructure::repl::Repl;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tui::infrastructure::runtime::Tui;
use crate::shared::infra::config::Settings;

/// The chosen frontend. Both drive the same `AgentLoop`; only the UI adapter differs.
pub enum App {
    Tui(Tui),
    Plain(Repl),
}

impl App {
    pub async fn run(self) -> Result<()> {
        match self {
            App::Tui(tui) => tui.run().await,
            App::Plain(mut repl) => repl.run().await,
        }
    }
}

/// The composition root: build the sandbox, the provider adapter, the tool registry and the agent
/// loop from resolved settings, then pick the frontend — the full-screen TUI on an interactive
/// terminal, or the plain line-based REPL when forced (`--plain`) or when stdout is not a TTY (piped
/// output, CI). This is the one place adapters are chosen.
pub fn wire(settings: Settings) -> Result<App> {
    let sandbox = Sandbox::new(&settings.path)?;
    let provider: Arc<dyn CompletionProvider> = Arc::new(OpenAiProvider::new(
        reqwest::Client::new(),
        settings.base_url,
        settings.api_key,
    ));
    let registry = ToolRegistry::new(default_fs_tools());
    let model = settings.model.clone();
    let agent_loop = AgentLoop::new(
        provider,
        registry,
        settings.model,
        settings.checkpoint_budget,
    );

    if settings.plain || !std::io::stdout().is_terminal() {
        Ok(App::Plain(Repl::new(
            agent_loop,
            sandbox,
            settings.system_prompt,
            settings.seed,
        )))
    } else {
        Ok(App::Tui(Tui::new(
            agent_loop,
            sandbox,
            settings.system_prompt,
            settings.seed,
            model,
        )))
    }
}
