use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use eventsource_stream::Eventsource;
use tokio_stream::StreamExt;

use crate::modules::provider::infrastructure::http_error::{bounded_preview, error_from_status};
use crate::shared::kernel::error::AgentError;

/// Enforced on the DECODED SSE payload by [`enforce_stream_budget`]. `read_timeout` resets on every
/// chunk, so it never bounds a provider that streams continuously.
pub(crate) const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;

/// Enforced on the RAW bytes, before `.eventsource()` framing. [`enforce_stream_budget`] only ever sees a
/// COMPLETE decoded event, so an endless line with no terminating blank line — never forming an event —
/// would grow unboundedly while the decoded check never fires (issue #31).
pub(crate) const MAX_RAW_STREAM_BYTES: usize = MAX_STREAM_BYTES;

/// Total turn deadline. `read_timeout` bounds only IDLE time between chunks and resets on every received
/// byte, so a provider trickling small chunks forever would never trip it (issue #31).
pub(crate) const MAX_STREAM_DURATION: Duration = Duration::from_secs(10 * 60);

/// A failed body read stays visible in the message rather than silently blanking the error.
pub(crate) async fn read_error_body(response: reqwest::Response) -> String {
    response
        .text()
        .await
        .unwrap_or_else(|error| format!("<error body unavailable: {error}>"))
}

pub(crate) async fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, AgentError> {
    let status = response.status();
    if !status.is_success() {
        return Err(error_from_status(status, read_error_body(response).await));
    }
    Ok(response)
}

pub(crate) async fn drain_sse(
    response: reqwest::Response,
    on_data: impl FnMut(&str) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    drain_sse_with_limits(response, MAX_STREAM_DURATION, MAX_RAW_STREAM_BYTES, on_data).await
}

/// `deadline` and `raw_cap` are injected so a test can exercise both ceilings without waiting out a real
/// 10-minute deadline or buffering 8 MiB.
async fn drain_sse_with_limits(
    response: reqwest::Response,
    deadline: Duration,
    raw_cap: usize,
    mut on_data: impl FnMut(&str) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    // A 200 that is not a stream is usually a misconfigured base URL: LM Studio answers an unknown route
    // with 200 + a JSON error, so `ensure_success` passes and `.eventsource()` yields no events, leaving
    // an empty turn the user only sees as "no content".
    // ponytail: only fires when the header is present AND clearly not a stream; a stream that omits the
    // Content-Type header (None) still drains normally, so a header-less compliant server is not rejected.
    if let Some(content_type) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        && !content_type.contains("event-stream")
    {
        let content_type = content_type.to_string();
        let body = read_error_body(response).await;
        return Err(AgentError::Provider(format!(
            "provider returned a non-stream response (content-type {content_type}); \
             check the base URL/model: {}",
            bounded_preview(&body)
        )));
    }

    // `take_while` can only end the stream, never error inline, so the total is rechecked after the drain
    // loop to turn "stream ended early" into a real error instead of a silent truncated success.
    let raw_total = Rc::new(Cell::new(0usize));
    let counter = Rc::clone(&raw_total);
    let raw_stream = response.bytes_stream().take_while(move |chunk| {
        let len = chunk.as_ref().map(|bytes| bytes.len()).unwrap_or(0);
        let total = counter.get().saturating_add(len);
        counter.set(total);
        total <= raw_cap
    });
    let stream = raw_stream.eventsource();
    tokio::pin!(stream);

    let drain = async {
        while let Some(event) = stream.next().await {
            let event = event
                .map_err(|error| AgentError::Provider(format!("error reading stream: {error}")))?;
            on_data(&event.data)?;
        }
        Ok::<(), AgentError>(())
    };

    match tokio::time::timeout(deadline, drain).await {
        Ok(result) => {
            result?;
            if raw_total.get() > raw_cap {
                return Err(AgentError::Provider(format!(
                    "provider stream exceeded the maximum raw response size ({raw_cap} bytes)"
                )));
            }
            Ok(())
        }
        Err(_) => Err(AgentError::Provider(format!(
            "provider stream exceeded the maximum turn duration ({deadline:?})"
        ))),
    }
}

/// Saturating, so the counter can never wrap past the cap.
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

/// `reason_is_cap` is compared per-adapter, so the protocol string stays out of this predicate.
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
        // Content-Length claims 1000 bytes but 5 are sent, then the connection closes.
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
    async fn drain_sse_surfaces_a_non_stream_200_body() {
        let body = r#"{"error":"Unexpected endpoint or method. (POST /chat/completions)"}"#;
        let response = serve_once(raw_with_body("200 OK", body)).await; // raw_with_body sets text/plain
        let error = drain_sse(response, |_data| Ok(()))
            .await
            .expect_err("a non-stream 200 body must fail the turn");
        assert!(matches!(error, AgentError::Provider(_)));
        assert!(
            error.to_string().contains("Unexpected endpoint or method"),
            "the provider body must be surfaced: {error}"
        );
    }

    #[tokio::test]
    async fn drain_sse_maps_a_read_error_to_provider() {
        // The chunk header claims 5 bytes but only 2 arrive before the connection closes.
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

    /// Headers only, then a stall. `read_timeout` never fires — the headers count as activity and no
    /// further chunk ever arrives to time out against — so only a total-turn deadline catches this.
    async fn serve_headers_then_stall(delay: Duration) -> reqwest::Response {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                                 Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(headers).await;
                let _ = stream.flush().await;
                tokio::time::sleep(delay).await;
                // The task ends without a chunk, dropping the connection.
            }
        });
        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        client.get(format!("http://{addr}/")).send().await.unwrap()
    }

    #[tokio::test]
    async fn drain_sse_with_limits_times_out_past_the_deadline() {
        let response = serve_headers_then_stall(Duration::from_secs(5)).await;
        let error = drain_sse_with_limits(
            response,
            Duration::from_millis(50),
            MAX_RAW_STREAM_BYTES,
            |_| Ok(()),
        )
        .await
        .expect_err("a stalled stream must time out");
        assert!(matches!(error, AgentError::Provider(_)));
        assert!(
            error.to_string().contains("maximum turn duration"),
            "got: {error}"
        );
    }

    #[tokio::test]
    async fn drain_sse_with_limits_errors_past_the_raw_cap_even_with_no_complete_event() {
        // No terminating blank line, so no complete SSE event ever forms.
        let payload = format!("data: {}", "a".repeat(200));
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{}",
            payload.len(),
            payload
        )
        .into_bytes();
        let response = serve_once(raw).await;
        let mut on_data_calls = 0;
        let error = drain_sse_with_limits(response, Duration::from_secs(5), 50, |_| {
            on_data_calls += 1;
            Ok(())
        })
        .await
        .expect_err("raw bytes past the cap, with no complete event ever formed, must still error");
        assert!(matches!(error, AgentError::Provider(_)));
        assert!(
            error.to_string().contains("maximum raw response size"),
            "got: {error}"
        );
        assert_eq!(
            on_data_calls, 0,
            "no complete SSE event ever formed, so on_data must never have been called"
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
