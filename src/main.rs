mod models;
mod services;
mod shared;

#[cfg(test)]
mod characterization;

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::models::chat::{ChatRequest, ChatTemplateKwargs, Message, Role};
use crate::services::chat::{CompletedTurn, StreamEvent, stream_completion};
use crate::services::sandbox::{Sandbox, expand_user_path, is_absolute_target};
use crate::services::tools::{ToolOutcome, confirmation_prompt, execute, tool_definitions};

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

const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const CLEAR_LINE: &str = "\r\x1b[K";
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

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
    let tools = tool_definitions();

    let client = reqwest::Client::new();
    let mut history: Vec<Message> = vec![Message::system(SYSTEM_PROMPT)];
    let mut reader = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = io::stdout();
    let is_tty = stdout.is_terminal();
    let mut seed = cli.prompt;

    writeln!(stdout, "workspace: {}", sandbox.root().display())?;

    'session: loop {
        let input = match seed.take() {
            Some(prompt) => prompt,
            None => {
                write!(stdout, "\nvocê › ")?;
                stdout.flush()?;
                match reader.next_line().await? {
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
            writeln!(stdout, "workspace: {}", sandbox.root().display())?;
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
                    writeln!(stdout, "workspace: {}", sandbox.root().display())?;
                }
                Err(error) => eprintln!("erro: {error:#}"),
            }
            continue;
        }

        history.push(Message::user(input));

        let mut checkpoint = Instant::now();
        loop {
            let request = ChatRequest {
                model: model.clone(),
                messages: history.clone(),
                stream: true,
                chat_template_kwargs: Some(ChatTemplateKwargs { thinking: true }),
                tools: tools.clone(),
            };

            let turn = match render_turn(&client, &api_key, &request, &mut stdout, is_tty).await {
                Ok(turn) => turn,
                Err(error) => {
                    eprintln!("erro: {error:#}");
                    // Roll back only a dangling user message (a first-round failure); a partial tool
                    // exchange is a valid, resumable state and is kept.
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
                let outcome = match confirmation_prompt(&sandbox, call) {
                    Some(confirmation) => {
                        write!(stdout, "{}", confirmation.prompt)?;
                        stdout.flush()?;
                        match reader.next_line().await? {
                            Some(answer)
                                if answer_approves(&answer, confirmation.default_accept) =>
                            {
                                execute(&sandbox, call)
                            }
                            Some(_) => ToolOutcome::Declined,
                            None => break 'session, // stdin closed at a prompt: end the session
                        }
                    }
                    None => execute(&sandbox, call),
                };
                history.push(Message::tool_result(
                    call.id.as_str(),
                    outcome.into_message_content(),
                ));
            }

            if checkpoint.elapsed() >= TOOL_CHECKPOINT {
                let minutes = checkpoint.elapsed().as_secs() / 60;
                write!(stdout, "Execução já dura ~{minutes}min. Continuar? [S/n] ")?;
                stdout.flush()?;
                match reader.next_line().await? {
                    Some(answer) if answer_approves(&answer, true) => checkpoint = Instant::now(),
                    Some(_) => break,
                    None => break 'session, // stdin closed at a prompt: end the session
                }
            }
        }
    }

    Ok(())
}

/// Stream one assistant turn: an ephemeral "pensando…" indicator while the model reasons, then the
/// answer token-by-token. Returns the assembled turn (content + any tool calls) for the loop to act on.
async fn render_turn(
    client: &reqwest::Client,
    api_key: &str,
    request: &ChatRequest,
    stdout: &mut io::Stdout,
    is_tty: bool,
) -> Result<CompletedTurn> {
    let started = Instant::now();
    let mut last_tick = started;
    let mut frame = 0usize;
    let mut answering = false;

    let result = stream_completion(client, BASE_URL, api_key, request, |event| {
        match event {
            StreamEvent::Reasoning(_) => {
                if !answering && is_tty && last_tick.elapsed() >= SPINNER_INTERVAL {
                    frame = (frame + 1) % SPINNER.len();
                    last_tick = Instant::now();
                    let secs = started.elapsed().as_secs();
                    write!(
                        stdout,
                        "{CLEAR_LINE}{DIM}{} pensando… ({secs}s){RESET}",
                        SPINNER[frame]
                    )?;
                    stdout.flush()?;
                }
            }
            StreamEvent::Content(text) => {
                if !answering {
                    answering = true;
                    if is_tty {
                        write!(stdout, "{CLEAR_LINE}")?;
                    }
                }
                write!(stdout, "{text}")?;
                stdout.flush()?;
            }
        }
        Ok(())
    })
    .await;

    // In a terminal, erase a leftover spinner if the stream ended or failed mid-reasoning, and
    // never leave the terminal dimmed. When piped, keep the output free of escape codes.
    if is_tty {
        if !answering {
            let _ = write!(stdout, "{CLEAR_LINE}");
        }
        let _ = write!(stdout, "{RESET}");
    }
    let _ = writeln!(stdout);
    let _ = stdout.flush();

    result
}

/// Interpret a confirmation answer. An explicit yes/no always wins; an empty or unrecognized answer
/// follows `default_accept` — `[S/n]` (accept) inside the workspace, `[s/N]` (decline) for out-of-root
/// operations.
fn answer_approves(answer: &str, default_accept: bool) -> bool {
    match answer.trim().to_lowercase().as_str() {
        "s" | "sim" | "y" | "yes" => true,
        "n" | "nao" | "não" | "no" => false,
        _ => default_accept,
    }
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} must be set (see .env)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_approves_follows_default_for_empty_and_unknown() {
        // Explicit yes/no override the default either way.
        for yes in ["s", "sim", "S", "y", "yes"] {
            assert!(answer_approves(yes, false), "{yes:?} should approve");
        }
        for no in ["n", "N", "nao", "não", "no", " NÃO "] {
            assert!(!answer_approves(no, true), "{no:?} should decline");
        }
        // Empty/unrecognized follow the default.
        assert!(answer_approves("", true));
        assert!(answer_approves("ok", true));
        assert!(!answer_approves("", false));
        assert!(!answer_approves("ok", false));
    }
}
