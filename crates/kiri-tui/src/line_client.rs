use std::collections::HashMap;

use anyhow::{Result, bail};
use kiri_core::protocol::{
    Connection, ContentBlock, ContentChunk, SessionId, SessionUpdate, ToolCallId, TurnEvent,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};

const CONNECTION_LOST: &str = "The connection to the Kiri daemon was lost.";
const NO_TOOLS_YET: &str = "Kiri has no tools yet, so this call did not run.\n";

// shortcut: a plain line client with no input during a turn, #4 replaces it with the inline interface.
pub async fn run(mut connection: Connection) -> Result<()> {
    let session = connection.new_session(&std::env::current_dir()?).await?;
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        stdout.write_all(b"> ").await?;
        stdout.flush().await?;
        let line = tokio::select! {
            line = lines.next_line() => line?,
            () = connection.closed() => bail!(CONNECTION_LOST),
        };
        let Some(line) = line else {
            return Ok(());
        };
        if !line.trim().is_empty() {
            run_turn(&mut connection, &session, &line, &mut stdout).await?;
        }
    }
}

async fn run_turn(
    connection: &mut Connection,
    session: &SessionId,
    prompt: &str,
    stdout: &mut Stdout,
) -> Result<()> {
    let lost = |_| anyhow::anyhow!(CONNECTION_LOST);
    connection
        .send_prompt(session, prompt)
        .await
        .map_err(lost)?;
    let mut printer = TurnPrinter {
        stdout,
        run: None,
        line_is_open: false,
        tool_names: HashMap::new(),
    };
    loop {
        match connection.next_turn_event().await.map_err(lost)? {
            TurnEvent::Update(update) => printer.print_update(*update).await?,
            TurnEvent::Ended(_) => return printer.print_ending().await,
            TurnEvent::Failed(message) => {
                printer.end_line().await?;
                eprintln!("error: {message}");
                return Ok(());
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Run {
    Reasoning,
    Text,
}

struct TurnPrinter<'output> {
    stdout: &'output mut Stdout,
    run: Option<Run>,
    line_is_open: bool,
    tool_names: HashMap<ToolCallId, String>,
}

impl TurnPrinter<'_> {
    async fn print_update(&mut self, update: SessionUpdate) -> Result<()> {
        match update {
            SessionUpdate::AgentThoughtChunk(chunk) => {
                self.print_chunk(Run::Reasoning, chunk).await
            }
            SessionUpdate::AgentMessageChunk(chunk) => self.print_chunk(Run::Text, chunk).await,
            SessionUpdate::ToolCall(tool_call) => {
                let line = format!("[tool call] {}\n", tool_call.title);
                self.tool_names
                    .insert(tool_call.tool_call_id, tool_call.title);
                self.print_tool_line(&line).await
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let name = self
                    .tool_names
                    .get(&update.tool_call_id)
                    .map_or("", String::as_str);
                let arguments = update
                    .fields
                    .raw_input
                    .as_ref()
                    .and_then(|raw| raw.as_str());
                let line = match arguments {
                    Some(arguments) if !arguments.is_empty() => {
                        format!("[tool call] {name} {arguments}\n")
                    }
                    _ => format!("[tool call] {name}\n"),
                };
                self.print_tool_line(&line).await
            }
            _ => Ok(()),
        }
    }

    async fn print_chunk(&mut self, kind: Run, chunk: ContentChunk) -> Result<()> {
        let ContentBlock::Text(text) = chunk.content else {
            return Ok(());
        };
        if self.run != Some(kind) {
            self.end_line().await?;
            if kind == Run::Reasoning {
                self.stdout.write_all(b"[reasoning] ").await?;
            }
            self.run = Some(kind);
        }
        self.stdout.write_all(text.text.as_bytes()).await?;
        self.stdout.flush().await?;
        self.line_is_open = !text.text.ends_with('\n');
        Ok(())
    }

    async fn print_tool_line(&mut self, line: &str) -> Result<()> {
        self.end_line().await?;
        self.run = None;
        self.stdout.write_all(line.as_bytes()).await?;
        self.stdout.flush().await?;
        Ok(())
    }

    async fn print_ending(&mut self) -> Result<()> {
        self.end_line().await?;
        if !self.tool_names.is_empty() {
            self.stdout.write_all(NO_TOOLS_YET.as_bytes()).await?;
        }
        Ok(())
    }

    async fn end_line(&mut self) -> Result<()> {
        if self.line_is_open {
            self.stdout.write_all(b"\n").await?;
            self.line_is_open = false;
        }
        Ok(())
    }
}
