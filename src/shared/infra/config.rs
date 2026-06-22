use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;

/// NVIDIA's OpenAI-compatible endpoint. Hardcoded for now; a future multi-provider feature will move
/// this into external configuration (see docs/decisions/0001-openai-compatible-provider.md).
const BASE_URL: &str = "https://integrate.api.nvidia.com/v1";

/// Seeded once as the first message of the session; shapes the assistant's identity, language,
/// concision, and code standards (see docs/decisions/0002-tool-calling-and-sandbox.md).
const SYSTEM_PROMPT: &str = concat!(
    "You are Kiri, a coding agent working in the user's local workspace through file tools (read, ",
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
#[command(
    name = "kiri",
    about = "Kiri — a directed coding-agent harness (NVIDIA OpenAI-compatible API)"
)]
struct Cli {
    /// Optional first message; the chat then continues interactively
    prompt: Option<String>,
    /// Sandbox root for file tools (also via KIRI_PATH; legacy T_CLI_PATH still honored).
    /// Defaults to the current directory.
    #[arg(long, env = "KIRI_PATH")]
    path: Option<PathBuf>,
    /// Use the plain line-based REPL instead of the full-screen TUI. The plain REPL is also used
    /// automatically when stdout is not a TTY (piped output, CI).
    #[arg(long)]
    plain: bool,
}

/// The resolved configuration the composition root needs to wire the harness. The API key and model
/// are read from the environment (loaded from `.env`); the key is never a CLI flag.
pub struct Settings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub system_prompt: &'static str,
    pub path: PathBuf,
    pub seed: Option<String>,
    pub checkpoint_budget: Duration,
    /// Force the plain line-based REPL even on a TTY (the `--plain` flag).
    pub plain: bool,
}

impl Settings {
    /// Load `.env`, parse the CLI, and read the required environment, failing fast with a clear error
    /// if `NVIDIA_API_KEY` or `NVIDIA_MODEL` is missing.
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();
        let cli = Cli::parse();
        let path = cli
            .path
            .or_else(|| std::env::var_os("T_CLI_PATH").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self {
            base_url: BASE_URL.to_string(),
            api_key: required_env("NVIDIA_API_KEY")?,
            model: required_env("NVIDIA_MODEL")?,
            system_prompt: SYSTEM_PROMPT,
            path,
            seed: cli.prompt,
            checkpoint_budget: TOOL_CHECKPOINT,
            plain: cli.plain,
        })
    }
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} must be set (see .env)"))
}
