mod modules;
mod shared;

#[cfg(test)]
mod characterization;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;

use crate::modules::agent::application::run_turn::RunTurn;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::infrastructure::openai::provider::OpenAiProvider;
use crate::modules::repl::infrastructure::repl::Repl;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::Sandbox;

/// NVIDIA's OpenAI-compatible endpoint. Hardcoded for now; a future multi-provider feature will
/// move this into external configuration (see docs/decisions/0001-openai-compatible-provider.md).
const BASE_URL: &str = "https://integrate.api.nvidia.com/v1";

/// Seeded once as the first message of the session; shapes the assistant's identity, language,
/// concision, and code standards (see docs/decisions/0002-tool-calling-and-sandbox.md).
const SYSTEM_PROMPT: &str = concat!(
    "You are T-Cli, a coding agent working in the user's local workspace through file tools (read, ",
    "write, edit, move, create, delete — files and directories). Relative paths resolve under the ",
    "active workspace root (the user moves it with /cd); to reach a file outside it, use an absolute ",
    "or '~/…' path, which the user must confirm explicitly. Prefer relative paths within the ",
    "workspace.\n\n",
    "- Before acting, state your plan in a sentence or two, then carry it out with the tools — ",
    "use them to do the work instead of describing it, chaining as many calls as needed. As you ",
    "go, describe each action in one short line, so your narration always matches what the tools ",
    "actually do. The CLI asks the user to approve each call; a call may be declined, so adapt.\n",
    "- If the request is ambiguous or underspecified, ask a clarifying question before acting ",
    "instead of guessing.\n",
    "- Stay grounded: read before you assert, never invent file contents or results, and report ",
    "failures honestly — never claim success a tool result didn't confirm.\n",
    "- Write senior-level code: secure, simple, explicit, well-named, self-documenting, and ",
    "always human-readable, with comments only for the non-obvious, matching the style of any ",
    "file you touch. Favor quality over quantity — do less, but do it well.\n",
    "- Reply in the user's language; keep code, identifiers, and file contents in English. Be ",
    "concise: no filler, no restating the task, and end with a short summary of what changed.\n",
    "- You are the assistant, not a demo — never narrate the session as a test or mention the harness.",
);

/// Wall-clock budget for a single user turn's tool loop before pausing to ask the user whether to keep
/// going. There is no fixed iteration cap — the loop runs until the model stops requesting tools — so
/// this time checkpoint is the only guard against an unattended runaway.
const TOOL_CHECKPOINT: Duration = Duration::from_secs(30 * 60);

#[derive(Parser)]
#[command(name = "t-cli", about = "Chat with NVIDIA's OpenAI-compatible API")]
struct Cli {
    /// Optional first message; the chat then continues interactively
    prompt: Option<String>,
    /// Sandbox root for file tools (also via T_CLI_PATH). Defaults to the current directory.
    #[arg(long, env = "T_CLI_PATH", default_value = ".")]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    let api_key = required_env("NVIDIA_API_KEY")?;
    let model = required_env("NVIDIA_MODEL")?;
    let sandbox = Sandbox::new(&cli.path)?;

    let provider: Arc<dyn CompletionProvider> = Arc::new(OpenAiProvider::new(
        reqwest::Client::new(),
        BASE_URL,
        api_key,
    ));
    let registry = ToolRegistry::new(default_fs_tools());
    let run_turn = RunTurn::new(provider, registry, model, TOOL_CHECKPOINT);

    let mut repl = Repl::new(run_turn, sandbox, SYSTEM_PROMPT, cli.prompt);
    repl.run().await
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} must be set (see .env)"))
}
