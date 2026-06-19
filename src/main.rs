mod models;
mod services;

use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::models::chat::{ChatRequest, ChatTemplateKwargs, Message, Role};
use crate::services::chat::{StreamEvent, stream_completion};

/// NVIDIA's OpenAI-compatible endpoint. Hardcoded for now; a future multi-provider feature will
/// move this into external configuration (see docs/decisions/0001-openai-compatible-provider.md).
const BASE_URL: &str = "https://integrate.api.nvidia.com/v1";

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
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    let api_key = required_env("NVIDIA_API_KEY")?;
    let model = required_env("NVIDIA_MODEL")?;

    let client = reqwest::Client::new();
    let mut history: Vec<Message> = Vec::new();
    let mut reader = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = io::stdout();
    let is_tty = stdout.is_terminal();
    let mut seed = cli.prompt;

    loop {
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

        history.push(Message {
            role: Role::User,
            content: input.to_string(),
        });

        let request = ChatRequest {
            model: model.clone(),
            messages: history.clone(),
            stream: true,
            chat_template_kwargs: Some(ChatTemplateKwargs { thinking: true }),
        };

        match render_turn(&client, &api_key, &request, &mut stdout, is_tty).await {
            Ok(answer) => history.push(Message {
                role: Role::Assistant,
                content: answer,
            }),
            Err(error) => {
                eprintln!("erro: {error:#}");
                history.pop();
            }
        }
    }

    Ok(())
}

/// Stream one assistant turn: an ephemeral "pensando…" indicator while the model reasons, then the
/// answer token-by-token. Returns the accumulated answer content for the conversation history.
async fn render_turn(
    client: &reqwest::Client,
    api_key: &str,
    request: &ChatRequest,
    stdout: &mut io::Stdout,
    is_tty: bool,
) -> Result<String> {
    let started = Instant::now();
    let mut last_tick = started;
    let mut frame = 0usize;
    let mut answering = false;
    let mut answer = String::new();

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
                answer.push_str(&text);
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

    result.map(|()| answer)
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} must be set (see .env)"))
}
