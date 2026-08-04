# 0001. Foundation stack

- Status: accepted
- Date: 2026-08-03

## Context

Kiri-V2 is a greenfield Rust terminal application. The stack must support a
responsive TUI, streaming provider responses, explicit failure handling, and a
small dependency surface. The exact stable Rust toolchain will be selected and
pinned when implementation starts.

## Decision

Kiri-V2 will use:

- Rust stable with edition 2024. The implementation starts from the latest
  stable release and pins the selected toolchain for reproducible builds.
- Tokio with an explicit multi-thread runtime. Blocking work must run through
  an appropriate blocking adapter such as `spawn_blocking`, never directly on
  asynchronous worker tasks.
- Ratatui with Crossterm for the TUI backend. Kiri owns the event loop and UI
  state.
- `ratatui-textarea` for multiline text editing. It remains a presentation
  concern and cannot leak into domain or application interfaces.
- Reqwest as the asynchronous HTTP client with Rustls, explicit timeouts, and
  streaming support.
- `eventsource-stream` for SSE framing. Reconnection is not automatic; provider
  adapters own provider-specific event parsing and accumulation.
- Serde and `serde_json` for serialization. Protocol and provider DTOs are
  separate from domain types. Dynamic JSON values are limited to genuinely
  dynamic boundaries such as tool schemas.
- Clap 4 with derive-based typed commands and validation.
- TOML with Serde for configuration files and an explicit Kiri configuration
  resolver. A generic configuration abstraction is not part of the foundation.

## Consequences

- The runtime, network, TUI, and configuration boundaries are explicit from the
  beginning.
- Provider-specific wire formats stay out of the domain model.
- Streaming requests do not silently retry and duplicate provider operations.
- The project accepts a small amount of adapter code in exchange for clear
  boundaries and predictable behavior.
- Performance-driven alternatives such as a SIMD JSON parser require a measured
  bottleneck before they can be introduced.

## Alternatives considered

- A single-thread Tokio runtime was rejected because the application has
  concurrent UI, provider streaming, and background work.
- Automatic SSE reconnection was rejected because one-shot model requests can
  duplicate work or side effects.
- A generic configuration framework was rejected because Kiri needs explicit
  precedence and validation rather than speculative abstraction.
- A second JSON parser was rejected until profiling demonstrates that Serde JSON
  is a real bottleneck.

## References

- [Rust 1.97.1 release](https://blog.rust-lang.org/2026/07/16/Rust-1.97.1/)
- [Tokio](https://tokio.rs/)
- [Ratatui backends](https://ratatui.rs/concepts/backends/)
- [ratatui-textarea](https://github.com/ratatui/ratatui-textarea)
- [Reqwest](https://github.com/seanmonstar/reqwest)
- [eventsource-stream](https://docs.rs/eventsource-stream/latest/eventsource_stream/)
- [Serde JSON](https://serde.rs/json.html)
- [Clap](https://github.com/clap-rs/clap)
- [TOML](https://github.com/toml-lang/toml)
