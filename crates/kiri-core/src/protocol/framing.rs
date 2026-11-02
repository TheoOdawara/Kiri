use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

const MAX_LINE_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<Vec<u8>>> {
    read_line_up_to(reader, MAX_LINE_BYTES).await
}

async fn read_line_up_to<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: u64,
) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    (&mut *reader)
        .take(max_bytes + 1)
        .read_until(b'\n', &mut line)
        .await?;
    if line.last() == Some(&b'\n') {
        line.pop();
        return Ok(Some(line));
    }
    if line.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("line over {max_bytes} bytes"),
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_line_at_the_limit_is_read_and_one_over_it_is_refused() {
        let mut at_limit: &[u8] = b"12345\n";
        assert_eq!(
            read_line_up_to(&mut at_limit, 5).await.unwrap().unwrap(),
            b"12345"
        );

        let mut over_limit: &[u8] = b"123456\n";
        let error = read_line_up_to(&mut over_limit, 5).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn a_closed_stream_ends_the_lines() {
        let mut closed: &[u8] = b"";
        assert_eq!(read_line(&mut closed).await.unwrap(), None);
    }
}
