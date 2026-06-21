mod modules;
mod shared;

#[cfg(test)]
mod characterization;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;

use crate::modules::agent::application::approval_policy::{Approval, ApprovalPolicy};
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::domain::completed_turn::CompletedTurn;
use crate::modules::agent::domain::message::Message;
use crate::modules::agent::domain::role::Role;
use crate::modules::provider::application::completion_provider::{CompletionProvider, TurnRequest};
use crate::modules::provider::infrastructure::openai::provider::OpenAiProvider;
use crate::modules::repl::infrastructure::terminal::Terminal;
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::application::tool::ToolOutcome;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::{
    Sandbox, expand_user_path, is_absolute_target,
};

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
    let mut sandbox = Sandbox::new(&cli.path)?;
    let registry = ToolRegistry::new(default_fs_tools());
    let tool_schemas = registry.schemas();

    let provider = OpenAiProvider::new(reqwest::Client::new(), BASE_URL, api_key);
    let mut history: Vec<Message> = vec![Message::system(SYSTEM_PROMPT)];
    let mut terminal = Terminal::new();
    let mut seed = cli.prompt;

    terminal.notice(&format!("workspace: {}", sandbox.root().display()));

    'session: loop {
        let input = match seed.take() {
            Some(prompt) => prompt,
            None => {
                terminal.prompt("\nvocê › ")?;
                match terminal.read_line().await? {
                    Some(line) => line,
                    None => break,
                }
            }
        };

        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if matches!(input, "/exit" | "/sair") {
            break;
        }
        if input == "/cd" {
            terminal.notice(&format!("workspace: {}", sandbox.root().display()));
            continue;
        }
        if let Some(arg) = input.strip_prefix("/cd ") {
            let arg = arg.trim();
            let target = if is_absolute_target(arg) {
                expand_user_path(arg)
            } else {
                sandbox.root().join(arg)
            };
            match Sandbox::new(&target) {
                Ok(new_sandbox) => {
                    sandbox = new_sandbox;
                    terminal.notice(&format!("workspace: {}", sandbox.root().display()));
                }
                Err(error) => eprintln!("erro: {error:#}"),
            }
            continue;
        }

        history.push(Message::user(input));

        let mut checkpoint = Instant::now();
        loop {
            let turn = match render_turn(&provider, &history, &model, &tool_schemas, &mut terminal)
                .await
            {
                Ok(turn) => turn,
                Err(error) => {
                    eprintln!("erro: {error:#}");
                    // Roll back only a dangling user message (a first-round failure); a partial
                    // tool exchange is a valid, resumable state and is kept.
                    if matches!(history.last(), Some(message) if message.role == Role::User) {
                        history.pop();
                    }
                    break;
                }
            };

            if turn.tool_calls.is_empty() {
                // Plain text turn (also covers a degenerate tool-call finish with no parsed calls).
                history.push(Message::assistant_text(turn.content));
                break;
            }

            let calls = turn.tool_calls;
            let narration = (!turn.content.is_empty()).then_some(turn.content);
            history.push(Message::assistant_tool_calls(narration, calls.clone()));

            for call in &calls {
                let outcome = match registry.confirm(&sandbox, call) {
                    Some(confirmation) => match terminal.decide(&confirmation).await {
                        Approval::Approved => registry.execute(&sandbox, call),
                        Approval::Declined => ToolOutcome::Declined,
                        Approval::Aborted => break 'session, // stdin closed at a prompt: end the session
                    },
                    None => registry.execute(&sandbox, call),
                };
                history.push(Message::tool_result(
                    call.id.as_str(),
                    outcome.into_message_content(),
                ));
            }

            if checkpoint.elapsed() >= TOOL_CHECKPOINT {
                let minutes = checkpoint.elapsed().as_secs() / 60;
                match terminal.confirm_continue(minutes).await {
                    Approval::Approved => checkpoint = Instant::now(),
                    Approval::Declined => break,
                    Approval::Aborted => break 'session, // stdin closed at a prompt: end the session
                }
            }
        }
    }

    Ok(())
}

/// Stream one assistant turn through the provider, rendering reasoning/content via the terminal, then
/// finishing the turn. Returns the assembled turn (content + any tool calls) for the loop to act on.
async fn render_turn(
    provider: &OpenAiProvider,
    messages: &[Message],
    model: &str,
    tools: &[serde_json::Value],
    terminal: &mut Terminal,
) -> Result<CompletedTurn> {
    terminal.begin_turn();
    let result = provider
        .complete(
            TurnRequest {
                messages,
                model,
                tools,
            },
            terminal,
        )
        .await;
    let _ = terminal.finish_turn();
    Ok(result?)
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} must be set (see .env)"))
}
