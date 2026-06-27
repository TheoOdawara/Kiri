# Audit — Provider module

> Scope: `src/modules/provider.rs` and all of `src/modules/provider/` — `application/{completion_provider,embedding_provider,secret_store}.rs`; `infrastructure/{factory,http_error,unconfigured}.rs`; `infrastructure/openai/{provider,wire,sse,message_dto,arguments,embeddings}.rs`; `infrastructure/anthropic/{provider,wire,sse,message_dto}.rs`; `infrastructure/secrets/{keyring_store,file_store}.rs` and the `*.rs` module files. Cross-checked `src/app.rs`, `src/modules/tui/infrastructure/runtime.rs`, and `src/shared/kernel/provider.rs` only to confirm callers/types.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
This is a high-quality module: clean ports/adapters split, secrets are wrapped in a zeroizing `Secret` and exposed only at the auth-header call site, error classification is centralized (`http_error`), and the test coverage of wire/SSE shapes and failure modes is genuinely strong (timeout fail-fast, in-band errors, keyless header omission, truncation flags). The headline issues are about parity and DRYness across the two streaming adapters rather than correctness of either in isolation: the Anthropic SSE accumulator is missing the unbounded-stream byte cap the OpenAI path explicitly added against untrusted provider responses (PROV-01); the send → status-check → body-read → stream-consume skeleton is copied across all three HTTP adapters (PROV-02); and a `#[allow(dead_code)]` on `SecretStore::delete` is now stale because the method is called in production (PROV-03). The rest are Low-severity duplication, doc-staleness, and magic-string nits.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 3 | 6 |

## Findings

### [PROV-01] Anthropic stream accumulator lacks the unbounded-response byte cap the OpenAI path enforces
- **Severity:** High
- **Category:** security
- **Location:** `src/modules/provider/infrastructure/anthropic/sse.rs:81` (the `TurnAccumulator` has no byte ceiling) vs `src/modules/provider/infrastructure/openai/sse.rs:17` and `src/modules/provider/infrastructure/openai/sse.rs:46`
- **Problem:** The OpenAI SSE path treats provider responses as untrusted and bounds the running total of streamed content + tool-call argument bytes by `MAX_STREAM_BYTES`, failing fast before memory grows without bound — its own comment notes that `read_timeout` only bounds idle time *between* chunks (it resets per chunk), so a server that streams continuously could OOM the process. The Anthropic adapter consumes its stream into `content`/`PartialToolUse.input` with no equivalent ceiling, so the identical threat (a misbehaving or compromised Messages endpoint streaming forever) is unguarded for one of the two adapters. The base URL is trusted (global config), so the realistic trigger is a buggy/compromised endpoint rather than a malicious repo — but the OpenAI adapter already deemed this guard necessary under the same model.
- **Evidence:**
```rust
// openai/sse.rs — present
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
accumulator.streamed_bytes = accumulator.streamed_bytes.saturating_add(delta_bytes);
if accumulator.streamed_bytes > MAX_STREAM_BYTES { return Err(/* ... */); }

// anthropic/sse.rs — absent
BlockDeltaDto::TextDelta { text } if !text.is_empty() => {
    accumulator.content.push_str(&text);   // no running-byte bound
    sink.on_event(StreamEvent::Content(text))
}
```
- **Recommendation:** Apply the same byte ceiling in `anthropic/sse.rs` (bound `content` + tool-input growth before absorbing each delta). Ideally lift the constant + the check into one shared place so both adapters cannot drift again (see PROV-02).

### [PROV-02] HTTP send / status-check / body-read / stream-consume skeleton duplicated across all three adapters
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/openai/provider.rs:82` (send + status + body-read, lines 82-96; stream loop lines 98-116), `src/modules/provider/infrastructure/anthropic/provider.rs:81` (send + status + body-read, lines 81-98; stream loop lines 100-118), `src/modules/provider/infrastructure/openai/embeddings.rs:85` (send + status + body-read, lines 85-103)
- **Problem:** The non-success preamble is byte-for-byte identical in three places: `let status = response.status(); if !status.is_success() { let body = response.text().await.unwrap_or_else(|error| format!("<error body unavailable: {error}>")); return Err(error_from_status(status, body)); }`. The two chat adapters additionally repeat the same `bytes_stream().eventsource()` + `tokio::pin!` + `while let Some(event)` + `error reading stream` + `handle_event` + `hit_empty_output_limit()` loop, differing only in which module's `handle_event`/`TurnAccumulator` they name. `http_error` already centralizes the *classification*; the read-and-classify and stream-drain steps around it should live with it too, so a fix (e.g. PROV-01's byte cap, or capping the error body) lands in one place.
- **Evidence:**
```rust
// repeated verbatim in openai/provider.rs, anthropic/provider.rs, openai/embeddings.rs
let status = response.status();
if !status.is_success() {
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("<error body unavailable: {error}>"));
    return Err(error_from_status(status, body));
}
```
- **Recommendation:** Add an `async fn ensure_success(response) -> Result<reqwest::Response, AgentError>` to `http_error` (consumes body + classifies on failure), and a small `stream_turn`-style helper parameterized over the per-provider accumulator/`handle_event` so the eventsource loop exists once. Do not implement now — scan only.

### [PROV-03] Stale `#[allow(dead_code)]` and outdated doc on `SecretStore::delete` (the method is live in production)
- **Severity:** Medium
- **Category:** dead-code
- **Location:** `src/modules/provider/application/secret_store.rs:16` (doc) and `src/modules/provider/application/secret_store.rs:18` (`#[allow(dead_code)]`)
- **Problem:** The port comment says `delete` is "Exercised by the file-store unit test and used by the `/provider` remove/logout flow (a later phase)" and the method carries `#[allow(dead_code)]`. Both are now wrong: `delete` is called through `Box<dyn SecretStore>` at boot (`src/app.rs:88`, clearing a stale key when the active provider is keyless) and on a live provider swap (`src/modules/tui/infrastructure/runtime.rs:221`). The annotation therefore suppresses a lint that would no longer fire and the comment misdescribes the contract. (Both call sites also do `let _ = secrets.delete(...)`; the keyring/file `delete` already maps "missing entry" to `Ok(())`, so the `let _` actually swallows only *genuine* delete failures — the "harmless no-op" justification understates that. Those swallow sites are out of this module's scope but corroborate that `delete` is live.)
- **Evidence:**
```rust
/// Exercised by the file-store unit test and used by the
/// `/provider` remove/logout flow (a later phase); kept here so the port models the full contract.
#[allow(dead_code)]
fn delete(&self, provider_id: &str) -> Result<(), AgentError>;
```
- **Recommendation:** Remove `#[allow(dead_code)]` and update the doc to point at the keyless-stale-key cleanup at boot/swap that now exercises it.

### [PROV-04] `align_embeddings` guarantees alignment by count only, not by a complete index permutation
- **Severity:** Medium
- **Category:** error-handling
- **Location:** `src/modules/provider/infrastructure/openai/embeddings.rs:58`
- **Problem:** The doc claims the function makes it so "a provider returning rows out of order or with a different count can never silently misalign vectors with texts." It only checks `data.len() == expected` and then stable-sorts by `index`. A response with duplicate/incomplete indices (e.g. two rows both `index: 0` for two inputs) passes the count check, sorts to `[0, 0]`, and returns input 0's vector for both inputs — input 1 silently gets the wrong embedding, which is exactly the recall corruption the guard exists to prevent. Real OpenAI/NVIDIA endpoints return contiguous indices, so likelihood is low, but the stated invariant has a hole.
- **Evidence:**
```rust
if data.len() != expected {
    return Err(/* count mismatch */);
}
data.sort_by_key(|datum| datum.index);          // [0,0] sorts to [0,0] — passes, but misaligned
Ok(data.into_iter().map(|datum| datum.embedding).collect())
```
- **Recommendation:** After sorting, verify each row's `index` equals its position (`datum.index == i`), erroring on any gap/duplicate; or fill a `Vec<Option<_>>` by index and reject if any slot is empty. Then the comment's guarantee holds.

### [PROV-05] Duplicated credential→key extraction (identical OAuth error string) and keyless auth-header omission
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/factory.rs:147` (`api_key_of`) and `src/modules/provider/infrastructure/factory.rs:163` (`optional_key`); `src/modules/provider/infrastructure/openai/provider.rs:79` and `src/modules/provider/infrastructure/openai/embeddings.rs:82`
- **Problem:** `api_key_of` and `optional_key` carry the same `Credential::Oauth(_)` arm with a byte-identical message ("has an OAuth credential, but Kiri only supports API-key credentials"); the two differ only in how they treat `Credential::None`. Separately, the "omit `Authorization` for a keyless endpoint, else `bearer_auth`" block is copied between the chat provider and the embeddings provider in the same `openai` module.
- **Evidence:**
```rust
// factory.rs, twice
Credential::Oauth(_) => Err(AgentError::Provider(format!(
    "provider '{}' has an OAuth credential, but Kiri only supports API-key credentials",
    profile.id
))),

// openai/provider.rs and openai/embeddings.rs
if let Some(key) = &self.api_key {
    request = request.bearer_auth(key.expose());
}
```
- **Recommendation:** Have `api_key_of` delegate to `optional_key` then map `Ok(None) -> Err(...)`, collapsing the OAuth arm to one site. Extract a tiny `fn with_optional_bearer(req, key: &Option<Secret>) -> RequestBuilder` shared by the two `openai` adapters.

### [PROV-06] In-band stream error messages are surfaced untruncated, unlike HTTP error bodies
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/provider/infrastructure/openai/sse.rs:76` (`format_stream_error`) and `src/modules/provider/infrastructure/anthropic/sse.rs:28` (the `Error` arm)
- **Problem:** `http_error::truncate_body` caps surfaced HTTP error bodies at `MAX_ERROR_BODY_CHARS` (600) precisely because the text is untrusted and may echo request content into the transcript. In-band SSE error messages (`{"error": {...}}` on a 200 stream) are formatted into the error verbatim with no bound, so an oversized provider error string reaches the transcript uncapped — an inconsistency in how the same module treats untrusted provider text.
- **Evidence:**
```rust
// openai/sse.rs
None => format!("stream error from provider: {message}"),   // message is untrusted, uncapped
// anthropic/sse.rs
StreamEventDto::Error { error } => Err(AgentError::Provider(format!(
    "stream error from provider: {} ({})", error.message, error.kind))),  // uncapped
```
- **Recommendation:** Route in-band error text through the same truncation helper used for HTTP bodies (or a shared bounded-preview function) so all provider-supplied error text is capped consistently.

### [PROV-07] `OpenAiProvider` doc comment is stale ("NVIDIA today")
- **Severity:** Low
- **Category:** naming
- **Location:** `src/modules/provider/infrastructure/openai/provider.rs:15`
- **Problem:** The struct doc says "OpenAI-compatible chat provider (NVIDIA today)." The factory now routes NVIDIA, OpenAI proper, generic OpenAI-compatible, custom, and keyless local (Ollama / LM Studio) endpoints through this one adapter (`factory.rs:72-83`), so the parenthetical is misleading about the adapter's actual reach.
- **Evidence:**
```rust
/// OpenAI-compatible chat provider (NVIDIA today). Holds the HTTP client and endpoint/credentials;
```
- **Recommendation:** Update the parenthetical to reflect the real set (NVIDIA / OpenAI / compatible / custom / keyless local), matching the accurate description already in `factory::build_provider`'s doc.

### [PROV-08] Env-derived API key is handled as a plain `String` (not zeroized) at the factory boundary
- **Severity:** Low
- **Category:** security
- **Location:** `src/modules/provider/infrastructure/factory.rs:120` (`api_key_from_env` returns `Option<String>`)
- **Problem:** Every other secret in the module lives in `Secret` (zeroized on drop, redacted in `Debug`). `api_key_from_env` returns the key as a bare `String`, so the credential exists as a non-zeroized heap copy from the env read until the caller (`app.rs:404`, `runtime.rs:154`) wraps it. The exposure is small (the value also already sits in the process environment in plaintext and the copy is short-lived), but it is an inconsistency with the module's own secret-handling discipline.
- **Evidence:**
```rust
pub fn api_key_from_env(profile: &ProviderProfile) -> Option<String> {
    // ...
    std::env::var(key).ok().filter(|value| !value.trim().is_empty())
}
```
- **Recommendation:** Return `Option<Secret>` so the key is wrapped at the moment it leaves the environment, keeping it out of any incidental `Debug`/log and zeroizing it on drop like every other credential.

### [PROV-09] Magic string literals for the canonical tool kind and truncation sentinels
- **Severity:** Low
- **Category:** magic
- **Location:** `src/modules/provider/infrastructure/openai/sse.rs:151` and `:182`, `src/modules/provider/infrastructure/anthropic/sse.rs:101` and `:131`
- **Problem:** The codebase otherwise names its boundaries as constants (`FIRST_PRINTABLE_ASCII`, `SSE_DONE_SENTINEL`, `MAX_OUTPUT_TOKENS`, `ANTHROPIC_VERSION`). Two recurring literals escape that discipline: the canonical tool kind `"function"` (defaulted in both `into_completed` implementations and in `message_dto`) and the truncation sentinels `"length"` (OpenAI `finish_reason`) / `"max_tokens"` (Anthropic `stop_reason`) compared inline in `hit_empty_output_limit`.
- **Evidence:**
```rust
// openai/sse.rs
self.finish_reason.as_deref() == Some("length")
kind: if partial.kind.is_empty() { "function".to_string() } else { partial.kind },
// anthropic/sse.rs
self.stop_reason.as_deref() == Some("max_tokens")
kind: "function".to_string(),
```
- **Recommendation:** Name them (`const FUNCTION_TOOL_KIND`, `const FINISH_REASON_LENGTH`, `const STOP_REASON_MAX_TOKENS`) so each comparison reads as intent and a typo cannot silently break truncation detection.

### [PROV-10] Test scaffolding duplicated across adapter test modules
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/modules/provider/infrastructure/openai/provider.rs:132` (`NullSink`), `src/modules/provider/infrastructure/anthropic/provider.rs:182` (`NullSink`), `src/modules/provider/infrastructure/unconfigured.rs:44` (`NullSink`); plus the near-identical loopback capture servers at `src/modules/provider/infrastructure/openai/provider.rs:247` and `src/modules/provider/infrastructure/openai/embeddings.rs:151`
- **Problem:** The same `struct NullSink; impl EventSink { fn on_event(...) -> Ok(()) }` is hand-written in three test modules, and the "bind loopback, capture request bytes, reply 400" server is duplicated between the chat-provider and embeddings tests. This is test-only, but it is the kind of scaffolding that should live once so a sink-trait or harness change touches one place.
- **Evidence:**
```rust
struct NullSink;
impl EventSink for NullSink {
    fn on_event(&mut self, _event: StreamEvent) -> Result<(), AgentError> { Ok(()) }
}
```
- **Recommendation:** Move a shared `NullSink`/`CollectSink` and a `capture_request` loopback helper into a small `#[cfg(test)]` support module under `provider/infrastructure` and reuse it from the adapter tests.

## Strengths
- Secret hygiene is exemplary: keys live in a zeroizing, `Debug`-redacted `Secret`, exposed only at the `bearer_auth`/`x-api-key` call sites, and the keyless path deliberately omits the header entirely (with a regression test) — a real reported LM Studio/Ollama bug locked down.
- Error handling is disciplined and well-tested: centralized 4xx-vs-transient classification with 429/408 carved out as retryable, in-band 200-stream errors surfaced instead of swallowed, output-cap truncation flagged rather than returning a phantom empty turn, and fail-fast read-timeout coverage proven with hermetic loopback servers.
- The `arguments` control-char escaper is a standout: a borrowed fast path for already-valid JSON, correct in-string vs structural-whitespace handling, send-side and receive-side boundary guards, and a fall-back-to-`{}` that prevents a garbled turn from poisoning later requests — all thoroughly tested.
