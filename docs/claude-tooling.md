# Recommended Claude Code Tooling

Curated MCPs, skills, and hooks for working on Kiri (async Rust agent harness, NVIDIA OpenAI-compatible API). Grounded in a focused
research pass; install nothing you don't need (lean by default).

## MCP servers

- **context7** (already connected) — **primary**. On-demand documentation for `clap`, `reqwest`, `tokio`,
  and `serde`. Use it before writing against any crate API instead of relying on memory.
- **github MCP** (available) — pull requests, issues, and the `security-debt` workflow. Activate once a
  GitHub remote exists.
- **rust-analyzer** — install the editor's rust-analyzer extension (VS Code / JetBrains) and use the IDE
  integration for live diagnostics. Zero extra config; complements the build gate.
  - Optional power-up (evaluate later, **not** installed in bootstrap): the `rust-analyzer-lsp` Claude Code
    plugin / `zircote/rust-lsp` — symbol navigation, automated refactors, and cargo-audit hooks. It overlaps
    the local fmt/clippy hook and adds config surface, so adopt it deliberately if IDE-grade navigation is
    wanted inside Claude Code.
- **docs.rs MCP** (`docsrs-mcp`, CrateDocs) — **skipped**: redundant with context7.

## Local hook

`.claude/settings.json` registers a PostToolUse hook on `Edit`/`Write`/`MultiEdit` that runs
`cargo fmt` (auto-format) then `cargo clippy --quiet` (passive feedback). If clippy-on-every-edit feels
slow, trim the hook command to `cargo fmt` only and rely on the gate/CI for clippy.

## Installed skills worth using here

- `requirements` — evolve `docs/requirements.md` as the CLI's scope firms up.
- `spec` / `spec-writing` — ADRs for cross-cutting decisions (e.g. config strategy, error model).
- `api-design` — when shaping the Ollama HTTP request/response contract.
- `testing-contract` — design the test contract before implementing each feature.
- `refactoring`, `performance-analysis`, `security-audit` / `audit` — quality + safety lenses.
- `code-review`, `closeout` — pre-commit review and the definition-of-done gate.
- `technical-writing` — README and human-facing docs.

## Sources

- Rust docs MCP servers: <https://crates.io/crates/docsrs-mcp>, <https://www.pulsemcp.com/servers/d6e-rust-docs>
- rust-analyzer for Claude Code: <https://claude.com/plugins/rust-analyzer-lsp>,
  <https://github.com/zircote/rust-lsp>, <https://github.com/zeenix/rust-analyzer-mcp>
