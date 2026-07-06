# ADR 0025 — Streaming: total-turn deadline and a raw pre-framing byte cap

- Status: Accepted
- Date: 2026-07-05
- Relates to: the existing `MAX_STREAM_BYTES` decoded-content ceiling in `streaming.rs`

## Context

Audited as issue #31. `streaming::drain_sse` had two gaps in the safety ceilings around a streamed
provider response:

1. **No total-turn deadline.** `read_timeout` on the shared HTTP client only bounds *idle* time between
   received chunks — it resets on every byte. A provider trickling small chunks continuously, forever
   (misbehaving, compromised, or simply pathological), would never trip it and could hold a turn open
   indefinitely.
2. **`MAX_STREAM_BYTES` is enforced on the DECODED SSE payload only**, via `enforce_stream_budget`, called
   from each adapter's `handle_event` on a *complete* parsed event's `data` field. A provider streaming an
   endless line with no terminating blank line — never forming a complete SSE event — would never reach
   that check at all. The raw bytes arriving over the wire (`bytes_stream()`, before `.eventsource()`
   framing) could grow without bound in `eventsource_stream`'s internal buffer while the decoded-content
   budget sat at zero, untripped.

## Decision

`drain_sse` now wraps its whole drain loop in `tokio::time::timeout(MAX_STREAM_DURATION, ...)` (10 minutes
— generous, well above any real single-turn generation, a safety ceiling like `MAX_STREAM_BYTES`, not a
user-facing setting) and separately counts RAW bytes as they arrive off `bytes_stream()`, via
`tokio_stream::StreamExt::take_while`, BEFORE they reach `.eventsource()`. The raw cap
(`MAX_RAW_STREAM_BYTES`) reuses `MAX_STREAM_BYTES`'s value: SSE framing overhead is negligible next to real
content, so there's no reason to allow more raw bytes through than the decoded budget already permits.

`take_while` ends the stream once the raw cap is passed rather than erroring inline (the closure is
synchronous, not able to inject an error into the `Result<Bytes, reqwest::Error>` item type it must
preserve for `.eventsource()` to keep accepting the stream). The raw byte counter (shared via
`Rc<Cell<usize>>` — safe here since `drain_sse`'s callers are `?Send` chat-provider adapters, never
`Send`-bound) is checked again after the drain loop exits, so "the stream ended early because the cap
tripped" becomes a real `AgentError`, not a silently truncated success.

Both ceilings stay fixed constants rather than `Settings`-configurable values (no new `[http]` TOML field,
no writer/merge-logic surface): they are safety ceilings analogous to the existing `MAX_STREAM_BYTES`,
which is likewise a hardcoded `const`, not a user preference — consistent with that established precedent
in the same file rather than introducing a new config-layer pattern for one more ceiling value.

The testable core is `drain_sse_with_limits(response, deadline, raw_cap, on_data)` — `drain_sse` (the
production entry point every adapter calls) fixes both parameters to the real constants; tests inject a
50ms deadline and a 50-byte raw cap so both new behaviors are exercised deterministically in milliseconds,
mirroring the injected-closure testability pattern already established for `resolve_credential_with_env`
(provider/factory.rs) and `scrub_env` (tools/exec.rs) — real constants are never mutated for a test, a
smaller value is passed instead.

## Consequences

- A provider that stalls mid-stream (no read_timeout trip, since idle-only) is now cut off after
  `MAX_STREAM_DURATION`.
- A provider that streams an endless, never-terminated line is now cut off after `MAX_RAW_STREAM_BYTES` raw
  bytes, even though no complete SSE event, and therefore no decoded-content check, ever fires.
- Locked by `drain_sse_with_limits_times_out_past_the_deadline` (a loopback server sends headers then
  stalls past a tiny injected deadline) and
  `drain_sse_with_limits_errors_past_the_raw_cap_even_with_no_complete_event` (a loopback server sends an
  unterminated `data: ...` line past a tiny injected raw cap, asserting `on_data` is never called — proving
  the decoded-content path genuinely never sees it, and only the raw cap catches it).
- No signature change for `drain_sse` itself — both existing call sites (`openai/provider.rs`,
  `anthropic/provider.rs`) are unaffected; the existing `enforce_stream_budget`/`MAX_STREAM_BYTES` guard on
  decoded content is untouched, and stays meaningful as defense-in-depth for the normal case (a
  well-formed stream whose decoded content, tracked separately per adapter, could in principle diverge
  slightly from a straight byte count).
- Closes #31.
