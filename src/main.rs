mod models;
mod services;

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::models::chat::{ChatRequest, ChatTemplateKwargs, Message, Role};
use crate::services::chat::{CompletedTurn, StreamEvent, stream_completion};
use crate::services::sandbox::Sandbox;
use crate::services::tools::{ToolOutcome, confirmation_prompt, execute, tool_definitions};

/// NVIDIA's OpenAI-compatible endpoint. Hardcoded for now; a future multi-provider feature will
/// move this into external configuration (see docs/decisions/0001-openai-compatible-provider.md).
const BASE_URL: &str = "https://integrate.api.nvidia.com/v1";

/// Upper bound on tool-call rounds per user turn; bounds a runaway agentic loop.
const MAX_TOOL_ITERATIONS: usize = 10;

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
    let sandbox = Sandbox::new(&cli.path)?;
    let tools = tool_definitions();

    let client = reqwest::Client::new();
    let mut history: Vec<Message> = Vec::new();
    let mut reader = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = io::stdout();
    let is_tty = stdout.is_terminal();
    let mut seed = cli.prompt;

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

        history.push(Message::user(input));

        let mut iterations = 0;
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

            iterations += 1;
            let calls = turn.tool_calls;
            let narration = (!turn.content.is_empty()).then_some(turn.content);
            history.push(Message::assistant_tool_calls(narration, calls.clone()));

            for call in &calls {
                let outcome = match confirmation_prompt(&sandbox, call) {
                    Some(prompt) => {
                        write!(stdout, "{prompt}")?;
                        stdout.flush()?;
                        match reader.next_line().await? {
                            Some(answer) if is_yes(&answer) => execute(&sandbox, call),
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

            if iterations >= MAX_TOOL_ITERATIONS {
                eprintln!("erro: limite de {MAX_TOOL_ITERATIONS} chamadas de ferramenta atingido");
                break;
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

fn is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_lowercase().as_str(),
        "s" | "sim" | "y" | "yes"
    )
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} must be set (see .env)"))
}
