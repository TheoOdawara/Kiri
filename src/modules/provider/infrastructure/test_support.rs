//! Shared `#[cfg(test)]` helpers for the provider SSE adapters.

use crate::modules::provider::application::completion_provider::EventSink;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::stream_event::StreamEvent;

/// An [`EventSink`] that records every streamed delta, so an SSE test can assert the exact live event
/// sequence the accumulator emitted. Shared by both chat adapters' test modules.
#[derive(Default)]
pub(crate) struct CollectSink(pub Vec<StreamEvent>);

impl EventSink for CollectSink {
    fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError> {
        self.0.push(event);
        Ok(())
    }
}

/// Start a loopback server that captures the first request's raw bytes (request line + headers + the
/// start of the body), replies `400` so the client returns promptly, and returns the captured text.
/// `drive` receives the server's base URL (`http://{addr}/v1`) and must issue exactly one request
/// against it — the assertion is on what was sent, not the response. Shared by the chat and embeddings
/// keyless/bearer regressions so the loopback plumbing lives in one place. Hermetic (loopback only),
/// bounded by an outer 5s guard so a wedged drive future cannot hang the suite.
pub(crate) async fn capture_request<F, Fut>(drive: F) -> String
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let captured = String::from_utf8_lossy(&buf[..n]).into_owned();
            let body = "stop";
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
            let _ = tx.send(captured);
        }
    });
    let base_url = format!("http://{addr}/v1");
    let _ = tokio::time::timeout(Duration::from_secs(5), drive(base_url)).await;
    tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("server should capture the request")
        .expect("capture channel should deliver")
}
