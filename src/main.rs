mod models;
mod services;

use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::Parser;

use crate::models::chat::{ChatRequest, Message, Role};
use crate::services::chat::stream_completion;

/// NVIDIA's OpenAI-compatible endpoint. Hardcoded for now; a future multi-provider feature will
/// move this into external configuration (see docs/decisions/0001-openai-compatible-provider.md).
const BASE_URL: &str = "https://integrate.api.nvidia.com/v1";

#[derive(Parser)]
#[command(name = "t-cli", about = "Chat with NVIDIA's OpenAI-compatible API")]
struct Cli {
    /// Prompt to send to the model
    prompt: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    let api_key = required_env("NVIDIA_API_KEY")?;
    let model = required_env("NVIDIA_MODEL")?;

    let client = reqwest::Client::new();
    let request = ChatRequest {
        model,
        messages: vec![Message {
            role: Role::User,
            content: cli.prompt,
        }],
        stream: true,
    };

    let mut stdout = io::stdout();
    stream_completion(&client, BASE_URL, &api_key, &request, |token| {
        stdout.write_all(token.as_bytes())?;
        stdout.flush()?;
        Ok(())
    })
    .await?;

    println!();
    Ok(())
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} must be set (see .env)"))
}
