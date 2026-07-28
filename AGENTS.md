# Kiri repository contract

This file adds repository-specific rules to the global working contract. `CLAUDE.md` contains historical
architecture detail; when it conflicts with this file or an accepted ADR, this file and the newer ADR win.

## Stack and checks

- Rust stable, edition 2024, single binary crate.
- Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo build`, then
  `cargo test --all-targets`.
- `unsafe` is forbidden. Do not add a native Win32 runtime or confinement fallback.

## Architecture

Kiri uses modular hexagonal architecture:
`src/modules/<context>/{domain,application,infrastructure}` with shared primitives under
`src/shared/{kernel,infra}`. Dependencies point inward. Ports live in `application`; adapters live in
`infrastructure`; `src/app.rs` is the composition root.

Windows ships a small native launcher only. The full runtime executes inside a supported WSL2 distribution
and uses the existing Linux `BwrapSandbox`; see ADR 0033. Windows-specific launch and lifecycle code stays
in `src/windows_launcher.rs`. Provider loopback bridging stays in `provider/infrastructure`.

## Git and language

- `main` is protected. Do not commit, push, or tag without explicit approval for that exact action.
- Chat is pt-BR. Documentation, code, identifiers, comments, and commit messages are English.
