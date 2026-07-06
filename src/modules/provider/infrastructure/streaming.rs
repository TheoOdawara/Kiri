//! The shared provider send/stream/body-read path. The non-success preamble (read the error body,
//! classify the status) is identical across all three HTTP adapters, and the SSE drain loop is identical
//! across the two chat adapters; the streamed-byte ceiling must be one number for both. They live here
//! once so a change to any of them reaches every adapter.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use eventsource_stream::Eventsource;
use tokio_stream::StreamExt;

use crate::modules::provider::infrastructure::http_error::error_from_status;
use crate::shared::kernel::error::AgentError;

/// Cap on the bytes a single streamed turn may accumulate (streamed content + reasoning + tool-call
/// arguments), enforced on the DECODED SSE payload by [`enforce_stream_budget`]. Provider responses are
/// untrusted input, and `read_timeout` only bounds idle time between chunks (it resets on each chunk), so
/// a misbehaving provider streaming continuously could otherwise grow memory without bound. The single
/// source both chat accumulators enforce. Generous — far above any real turn — purely a safety ceiling.
pub(crate) const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;

/// Cap on the RAW bytes `drain_sse` reads off the wire (`bytes_stream()`, before `.eventsource()`
/// framing), enforced independently of [`MAX_STREAM_BYTES`]. Necessary because [`enforce_stream_budget`]
/// only ever sees a COMPLETE decoded event's `data` payload — a provider streaming an endless line with no
/// terminating blank line (never forming a complete SSE event) would have raw bytes accumulate
/// unboundedly while the decoded-content check never fires at all (issue #31). Same value as
/// `MAX_STREAM_BYTES`: SSE framing overhead (a `data: ` prefix and blank-line delimiters per event) is
/// negligible next to real content, so there is no reason to allow more raw bytes through than the
/// decoded budget already permits.
pub(crate) const MAX_RAW_STREAM_BYTES: usize = MAX_STREAM_BYTES;

/// Ceiling on how long a single streamed turn may run in total, from the first byte to the last.
/// `read_timeout` on the shared HTTP client only bounds IDLE time between chunks — it resets on every
/// received byte — so a provider trickling small chunks continuously, forever, would never trip it
/// (issue #31). Generous, well above any real single-turn generation; purely a safety ceiling, like
/// `MAX_STREAM_BYTES`.
pub(crate) const MAX_STREAM_DURATION: Duration = Duration::from_secs(10 * 60);

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
/// so the per-provider event handling is all that stays in each adapter. Bounded by [`MAX_STREAM_DURATION`]
/// (total turn deadline) and [`MAX_RAW_STREAM_BYTES`] (raw pre-framing byte cap) — see [`drain_sse_with_limits`].
pub(crate) async fn drain_sse(
    response: reqwest::Response,
    on_data: impl FnMut(&str) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    drain_sse_with_limits(response, MAX_STREAM_DURATION, MAX_RAW_STREAM_BYTES, on_data).await
}

/// The testable core of [`drain_sse`]: `deadline` and `raw_cap` are injected so both ceilings can be
/// exercised with small, fast values in tests instead of waiting out a real 10-minute deadline or
/// buffering 8 MiB of raw bytes. Production calls `drain_sse`, which fixes both to their real constants.
async fn drain_sse_with_limits(
    response: reqwest::Response,
    deadline: Duration,
    raw_cap: usize,
    mut on_data: impl FnMut(&str) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    // Counts raw bytes as they arrive off the wire, BEFORE `.eventsource()` ever assembles them into a
    // complete event — `take_while` ends the stream (rather than erroring inline) once the cap is passed,
    // so the raw total is checked again after the drain loop to turn "stream ended early" into a real
    // error instead of a silent, truncated success.
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

    /// Start a loopback server that sends only response headers, waits `delay`, then closes without ever
    /// sending a body. Models a provider that stops trickling mid-stream: `read_timeout` never fires
    /// (headers count as activity, and no further chunk ever arrives to reset an idle timer against), so
    /// only a total-turn deadline catches it.
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
                // Task ends here without sending a chunk; the connection drops.
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
        // Issue #31: a provider trickling nothing for longer than the turn deadline must be cut off, even
        // though `read_timeout` (which only bounds IDLE time between chunks) never fires here — the server
        // sends headers (activity) then genuinely stalls.
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
        // Issue #31: an endless line with no terminating blank line never forms a complete SSE event, so
        // `enforce_stream_budget` (which only sees a decoded `data` payload) would never fire — only a
        // cap on the RAW pre-framing bytes catches this.
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
