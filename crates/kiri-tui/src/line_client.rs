use anyhow::{Result, bail};
use kiri_core::protocol::{Connection, ContentBlock, SessionId, SessionUpdate, TurnEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};

const CONNECTION_LOST: &str = "The connection to the Kiri daemon was lost.";

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
    let mut line_is_open = false;
    loop {
        match connection.next_turn_event().await.map_err(lost)? {
            TurnEvent::Update(update) => {
                if let SessionUpdate::AgentMessageChunk(chunk) = *update
                    && let ContentBlock::Text(text) = chunk.content
                {
                    stdout.write_all(text.text.as_bytes()).await?;
                    stdout.flush().await?;
                    line_is_open = !text.text.ends_with('\n');
                }
            }
            TurnEvent::Ended(_) => break,
            TurnEvent::Failed(message) => {
                end_line(stdout, line_is_open).await?;
                eprintln!("error: {message}");
                return Ok(());
            }
        }
    }
    end_line(stdout, line_is_open).await
}

async fn end_line(stdout: &mut Stdout, line_is_open: bool) -> Result<()> {
    if line_is_open {
        stdout.write_all(b"\n").await?;
    }
    Ok(())
}
