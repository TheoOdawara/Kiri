use crate::modules::provider::application::completion_provider::EventSink;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::stream_event::StreamEvent;

#[derive(Default)]
pub(crate) struct CollectSink(pub Vec<StreamEvent>);

impl EventSink for CollectSink {
    fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError> {
        self.0.push(event);
        Ok(())
    }
}

/// Captures the first request's raw bytes and returns them; the server replies `400` so the client
/// returns promptly. `drive` receives the base URL and must issue exactly one request against it.
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
