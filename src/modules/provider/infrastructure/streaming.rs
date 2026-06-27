//! The shared provider send/stream/body-read path. The non-success preamble (read the error body,
//! classify the status) is identical across all three HTTP adapters, and the SSE drain loop is identical
//! across the two chat adapters; the streamed-byte ceiling must be one number for both. They live here
//! once so a change to any of them reaches every adapter.

use eventsource_stream::Eventsource;
use tokio_stream::StreamExt;

use crate::modules::provider::infrastructure::http_error::error_from_status;
use crate::shared::kernel::error::AgentError;

/// Cap on the bytes a single streamed turn may accumulate (streamed content + reasoning + tool-call
/// arguments). Provider responses are untrusted input, and `read_timeout` only bounds idle time between
/// chunks (it resets on each chunk), so a misbehaving provider streaming continuously could otherwise
/// grow memory without bound. The single source both chat accumulators enforce through
/// [`enforce_stream_budget`]. Generous — far above any real turn — purely a safety ceiling.
pub(crate) const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;

/// Read a response body for diagnostics. The status drives the error path; the body is only diagnostic,
/// so if reading it fails the failure stays visible in the message rather than being silently blanked.
pub(crate) async fn read_error_body(response: reqwest::Response) -> String {
    response
        .text()
        .await
        .unwrap_or_else(|error| format!("<error body unavailable: {error}>"))
}

/// Return the response unchanged on a 2xx; otherwise classify the status (with a bounded body) into the
/// matching [`AgentError`]. The one non-success preamble all three HTTP adapters call.
pub(crate) async fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, AgentError> {
    let status = response.status();
    if !status.is_success() {
        return Err(error_from_status(status, read_error_body(response).await));
    }
    Ok(response)
}

/// Drain an SSE response, feeding each event's `data` payload to `on_data`. Owns the
/// `bytes_stream().eventsource()` framing, the pin, the read loop, and the `error reading stream` mapping,
/// so the per-provider event handling is all that stays in each adapter.
pub(crate) async fn drain_sse(
    response: reqwest::Response,
    mut on_data: impl FnMut(&str) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    let stream = response.bytes_stream().eventsource();
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        let event = event
            .map_err(|error| AgentError::Provider(format!("error reading stream: {error}")))?;
        on_data(&event.data)?;
    }
    Ok(())
}

/// Add `delta_bytes` to the running streamed-byte total and fail fast past [`MAX_STREAM_BYTES`].
/// Saturating so the counter can never wrap. Both chat accumulators call this, so exactly one ceiling
/// exists for the whole provider layer.
pub(crate) fn enforce_stream_budget(
    running: &mut usize,
    delta_bytes: usize,
) -> Result<(), AgentError> {
    *running = running.saturating_add(delta_bytes);
    if *running > MAX_STREAM_BYTES {
        return Err(AgentError::Provider(format!(
            "provider stream exceeded the maximum response size ({MAX_STREAM_BYTES} bytes)"
        )));
    }
    Ok(())
}

/// Whether a turn ended truncated with no usable output: the stop/finish reason was the output-token cap
/// AND nothing (content or tool calls) was produced. Each adapter passes its own sentinel comparison via
/// `reason_is_cap`, so the predicate is shared while the protocol string stays per-provider.
pub(crate) fn is_empty_truncation(
    reason_is_cap: bool,
    content: &str,
    tool_calls_empty: bool,
) -> bool {
    reason_is_cap && content.is_empty() && tool_calls_empty
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Start a loopback server that replies once with a full raw HTTP/1.1 response, then GET it and return
    /// the `reqwest::Response`. The server drains the request line before replying. Hermetic (loopback).
    async fn serve_once(raw_response: Vec<u8>) -> reqwest::Response {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(&raw_response).await;
                let _ = stream.flush().await;
            }
        });
        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        client.get(format!("http://{addr}/")).send().await.unwrap()
    }

    fn raw_with_body(status_line: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn ensure_success_passes_through_a_2xx() {
        let response = serve_once(raw_with_body("200 OK", "ok")).await;
        let response = ensure_success(response)
            .await
            .expect("2xx must pass through");
        assert_eq!(response.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn ensure_success_classifies_a_4xx_with_its_body() {
        let response = serve_once(raw_with_body("400 Bad Request", "invalid model: nope")).await;
        match ensure_success(response).await {
            Err(AgentError::ProviderRejected { status, body }) => {
                assert_eq!(status, 400);
                assert!(body.contains("invalid model"), "body lost: {body:?}");
            }
            other => panic!("expected ProviderRejected(400), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ensure_success_classifies_a_5xx() {
        let response = serve_once(raw_with_body("500 Internal Server Error", "boom")).await;
        assert!(matches!(
            ensure_success(response).await,
            Err(AgentError::Provider(_))
        ));
    }

    #[tokio::test]
    async fn read_error_body_reports_unavailable_when_the_body_read_fails() {
        // Content-Length claims more bytes than are sent, then the connection closes: the body read fails
        // (incomplete message), so read_error_body returns its placeholder rather than blanking the error.
        let raw =
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nshort"
                .to_vec();
        let response = serve_once(raw).await;
        let body = read_error_body(response).await;
        assert!(body.contains("error body unavailable"), "got: {body}");
    }

    #[tokio::test]
    async fn drain_sse_feeds_each_data_payload_to_the_handler() {
        let sse = "data: a\n\ndata: b\n\n";
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            sse.len(),
            sse
        )
        .into_bytes();
        let response = serve_once(raw).await;
        let mut seen = Vec::new();
        drain_sse(response, |data| {
            seen.push(data.to_string());
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn drain_sse_maps_a_read_error_to_provider() {
        // A chunked body whose chunk header claims 5 bytes but only 2 arrive before the connection closes:
        // the underlying stream surfaces a decode error, which drain_sse maps to a Provider error.
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nab"
                .to_vec();
        let response = serve_once(raw).await;
        let error = drain_sse(response, |_data| Ok(()))
            .await
            .expect_err("an incomplete stream must error");
        assert!(matches!(error, AgentError::Provider(_)));
        assert!(
            error.to_string().contains("error reading stream"),
            "got: {error}"
        );
    }

    #[test]
    fn enforce_stream_budget_allows_within_the_cap() {
        let mut running = 0usize;
        assert!(enforce_stream_budget(&mut running, 1024).is_ok());
        assert_eq!(running, 1024);
    }

    #[test]
    fn enforce_stream_budget_errors_past_the_cap() {
        let mut running = MAX_STREAM_BYTES;
        let error =
            enforce_stream_budget(&mut running, 1).expect_err("one byte past the cap must fail");
        assert!(error.to_string().contains("maximum response size"));
    }

    #[test]
    fn enforce_stream_budget_saturates_without_overflow() {
        let mut running = usize::MAX - 1;
        // The saturating add must not panic on overflow; it errors past the cap instead.
        let error =
            enforce_stream_budget(&mut running, usize::MAX).expect_err("must error, not overflow");
        assert!(error.to_string().contains("maximum response size"));
        assert_eq!(running, usize::MAX);
    }

    #[test]
    fn is_empty_truncation_truth_table() {
        assert!(is_empty_truncation(true, "", true));
        assert!(!is_empty_truncation(false, "", true));
        assert!(!is_empty_truncation(true, "x", true));
        assert!(!is_empty_truncation(true, "", false));
    }
}
