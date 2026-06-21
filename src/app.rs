use std::sync::Arc;

use anyhow::Result;

use crate::modules::agent::application::run_turn::RunTurn;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::infrastructure::openai::provider::OpenAiProvider;
use crate::modules::repl::infrastructure::repl::Repl;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::shared::infra::config::Settings;

/// The composition root: build the sandbox, the provider adapter, the tool registry and the agent
/// loop from resolved settings, and inject them into the REPL driving adapter. This is the one place
/// adapters are chosen — adding a second provider is a change here only.
pub fn wire(settings: Settings) -> Result<Repl> {
    let sandbox = Sandbox::new(&settings.path)?;
    let provider: Arc<dyn CompletionProvider> = Arc::new(OpenAiProvider::new(
        reqwest::Client::new(),
        settings.base_url,
        settings.api_key,
    ));
    let registry = ToolRegistry::new(default_fs_tools());
    let run_turn = RunTurn::new(
        provider,
        registry,
        settings.model,
        settings.checkpoint_budget,
    );
    Ok(Repl::new(
        run_turn,
        sandbox,
        settings.system_prompt,
        settings.seed,
    ))
}
