use anyhow::{Result, bail};
use kiri_core::protocol::Connection;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// shortcut: shows the prompt and ignores what is typed, #16 replaces this with the line client.
pub async fn run(mut connection: Connection) -> Result<()> {
    let mut stdout = tokio::io::stdout();
    stdout.write_all(b"> ").await?;
    stdout.flush().await?;

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                if line?.is_none() {
                    return Ok(());
                }
            }
            () = connection.closed() => bail!("The connection to the Kiri daemon was lost."),
        }
    }
}
