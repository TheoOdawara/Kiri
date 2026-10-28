# 0002. Stack: every dependency, and what stays in the standard library
- Status: accepted
- Date: 2026-10-27

## Context

[ADR 0001](0001-architecture.md) settled the shape: a per-user daemon, ACP at the boundary, three crates,
modules by capability. This ADR settles what the code is built from, one dependency at a time, before the
first crate exists.

The forces, from `docs/requirements/` (SRS 3.0.0):

- One maintainer. Every dependency is an API someone else changes, so one enters only when writing and
  keeping the code ourselves costs more.
- Linux, macOS, and Windows are native, each with its own sandbox (FR-SAF-07, Milestone 1).
- The install script is the only way to install (FR-DST-03). Nobody but this repository compiles Kiri,
  which frees the minimum Rust version and the Linux target.
- Every dependency is the latest stable release, pinned exactly.

Research on 2026-10-27 read the manifests of Codex (`openai/codex`) and Grok Build (`xai-org/grok-build`),
and the docs of Claude Code, Antigravity, Gemini CLI, OpenCode, and Kimi Code.

## Decision

### Toolchain and targets

- Rust 1.99.1, pinned in `rust-toolchain.toml` with `rustfmt` and `clippy`; edition 2024;
  `rust-version = "1.99"`. A new Rust release is a `chore` PR with the gates green.
- Release targets: `x86_64` and `aarch64` of `unknown-linux-musl`, `apple-darwin`, and `pc-windows-msvc`.
  Linux is static musl, so one binary runs on any distribution; on that target only, the global allocator
  is jemalloc, since musl's allocator contends under the daemon's threads.

### Dependencies

| Area | Crate | Version | Settings |
| --- | --- | --- | --- |
| Async runtime | `tokio` | 1.54.1 | |
| Errors | `thiserror` in `kiri-core`; `anyhow` in `kiri` and `kiri-tui` | 2.0.21; 1.0.104 | |
| Logs | `tracing`, `tracing-subscriber` | 0.1.44, 0.3.23 | JSONL output; rotation is our code |
| CLI | `clap` | 4.6.7 | `derive` |
| Serialization | `serde`, `serde_json` | 1.0.229, 1.0.154 | |
| ACP types | `agent-client-protocol-schema` | 1.13.0 | Types only; the JSON-RPC layer is our code |
| OS APIs | `libc`; `windows-sys` | 0.2.191; 0.61.2 | No `windows` crate unless a spec finds an API `windows-sys` lacks |
| Configuration | `toml_edit` | 0.25.17 | `serde`; edits keep the user's comments and layout |
| HTTP | `reqwest` | 0.13.5 | `rustls` with `aws-lc-rs` and `rustls-platform-verifier`, `http2`, `system-proxy`, `stream` |
| SSE | `eventsource-stream` | 0.2.3 | |
| Permission globs | `globset` | 0.4.21 | |
| Diffs | `similar` | 3.2.1 | |
| Linux sandbox | `seccompiler` | 0.5.0 | With `bwrap`; Landlock cannot deny connecting to the daemon's socket |
| macOS keychain | `keyring-core`, `apple-native-keyring-store` | 1.0.1, 1.0.2 | macOS only |
| Terminal | `crossterm` | 0.29.0 | `event-stream` |
| Rendering | `ratatui` | 0.30.2 | No default features; crossterm backend; our own inline terminal |
| Markdown | `pulldown-cmark` | 0.13.5 | No default features; GitHub extensions turned on at run time |
| Syntax highlight | `syntect`; `two-face` | 5.3.0; =0.5.2 | Oniguruma (`parsing`, `regex-onig`; `syntect-onig`); a theme built from the brand palette |
| Text width | `unicode-width`, `unicode-segmentation` | 0.2.3, 1.13.3 | The tables `ratatui` already uses |
| Clipboard | `arboard` | 3.7.0 | `wayland-data-control` |
| Images | `image` | 0.25.10 | No default features; `png`, `jpeg`, `webp`, `gif` |
| MCP | `rmcp` | 3.8.0 | No default features; `client`, `server`, `transport-child-process`, `transport-io`, `transport-streamable-http-client-reqwest`, `auth` |
| OAuth | `oauth2` | 5.0.0 | Already present through `rmcp`'s `auth` |
| Tool schemas | `schemars` | 1.2.3 | `derive`; one adjustment pass per provider |
| Allocator | `tikv-jemallocator` | 0.7.0 | Linux musl only |
| Update | `minisign-verify`, `self-replace`, `flate2`, `tar`, `zip` | 0.3.0, 1.5.1, 1.1.10, 0.4.46, 9.1.3 | `tar` without `xattr`; `zip` with `deflate` only |
| Tests only | `insta`, `wiremock` | 1.49.0, 0.6.5 | TUI and SSE snapshots; fake provider APIs |

### Our own code, on the standard library

- The JSON-RPC layer under ACP, over the daemon's socket and named pipe. Owner-only access uses file
  permissions and peer credentials on Unix, and a DACL on Windows.
- Killing a process tree: process groups and `killpg` on Unix, a Job Object on Windows.
- The macOS sandbox: an SBPL profile generated for `/usr/bin/sandbox-exec`, which also denies the daemon's
  socket.
- The Windows sandbox, the model Claude Code's `sandbox-runtime` and Codex share: a dedicated local
  account, a WFP filter keyed on its SID, a write-restricted token, and ACEs on the working tree, set up
  once under UAC.
- Credentials: the macOS Keychain, falling back to `~/.kiri/credentials.json` at mode `0600`; the same file
  on Linux; on Windows, the same file encrypted with DPAPI. `credentials.store = "file"` forces the file on
  macOS. A provider's environment variables take precedence over a stored credential. This settles OQ-01.
- Token estimates: the provider's reported usage, and bytes divided by 4 with a margin before a request.
- The home directory (`std::env::home_dir`), IDs (UUIDv7 from the OS random source), and dates (UTC
  RFC 3339 and HTTP dates; local time from the OS).
- The OAuth callback listener and opening the browser.
- Shell commands run through pipes, with stdin closed. There is no pseudo-terminal.
- The vim editor of the prompt, as a `ratatui` widget.
- In tests: temporary directories and running the `kiri` binary.

### Programs Kiri runs

- `rg` 15.2.0, shipped in the release archive next to `kiri`. The system's `rg` is a configuration option.
- `bwrap` on Linux, the system's or the one the release ships.
- The system's `git`, for the `Later` features that clone or fetch: plugins and skill upstreams.
- The user's `claude` CLI, for the Claude subscription (FR-PRV-06).

### Release tooling (CI only, not in the binary)

- macOS: `rcodesign`, signing with one fixed certificate of our own so the Keychain keeps recognizing Kiri
  across updates. A spike confirms it before Distribution; the fallback is a Developer ID.
- Windows: no Authenticode signature at first. Microsoft's Artifact Signing does not accept individuals
  outside the USA and Canada.
- Every archive is signed with minisign; the updater verifies the signature, and the install script
  checks the SHA-256 over HTTPS.

## Consequences

Easier:
- About forty crates, each with a known reason, against roughly 110 in Grok Build's workspace. Grok
  Build's stack matches this one almost item by item, so the choices are proven in production.
- Six areas that usually take a dependency (IDs, dates, home directory, the JSON-RPC layer, temporary
  directories, binary tests) take none.
- Musl and the install script remove glibc versions and package-manager variants from support.

Harder, and accepted:
- The Windows sandbox is the largest piece of Milestone 1 and needs a one-time elevated setup.
- Two JSON-RPC stacks: ours for ACP and `rmcp`'s for MCP.
- Oniguruma and jemalloc compile C in the build, as `aws-lc-rs` already does.
- `crossterm` releases slowly; Codex carries four fixes in a fork. The TUI spec checks those cases.
- Without a code signature, a macOS update can ask once for Keychain access, and Smart App Control can
  block `kiri.exe` on Windows. Both wait on the Distribution spikes.

## Alternatives considered

- **`dirs`, `uuid`, `chrono` or `jiff`, `tempfile`, `assert_cmd`** — each replaces a few dozen lines
  the standard library and the OS already cover.
- **The keyring on all three systems (Codex)** — Linux's Secret Service fails over SSH, on servers, and in
  WSL, so the file fallback is needed anyway.
- **Landlock on Linux** — cannot block connecting to a Unix socket, the daemon's included.
- **A restricted token at Low integrity, or AppContainer, on Windows** — the first blocks neither reads nor
  the network, and leaves a Low label on the project; the second needs Windows 11 24H2 (Antigravity) and
  read grants on every toolchain directory.
- **termion; termwiz; our own input parser** — no Windows; built for WezTerm; months spent on escape
  sequences.
- **Our own renderer (Ink's role in Claude Code)** — rewrites buffers, diffs, and wrapping, and again for
  the full-screen interface.
- **`comrak`; tree-sitter highlighting; syntect with `fancy-regex`** — a tree per 10 ms batch; a C grammar
  per language; Sublime grammars that `fancy-regex` cannot read.
- **A pseudo-terminal (`portable-pty`)** — ConPTY under the sandbox account on Windows, and ANSI
  escapes in what the model reads; no requirement asks for interactive sessions.
- **`gix`; `git2`** — 0.x with a moving API; C, with SSH and TLS set up on musl and Windows. The system's
  `git` brings the user's credentials.
- **MCP over our own JSON-RPC** — streamable HTTP, remote OAuth, and every spec revision become ours.
- **A gnu Linux binary** — fails on distributions older than the build machine's.
