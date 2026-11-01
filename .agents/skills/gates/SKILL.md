---
name: gates
description: The Kiri repository's gates, cheapest first, with their exact invocations, and how to serve the docs site. Use before calling any work done, in /tightship:closeout, and when building or serving the docs.
---

# gates

Run every gate from the repository root: Zensical reads `zensical.toml` only from the current folder.

| Gate | Invocation | Exit |
| --- | --- | --- |
| Formatting | `cargo fmt --all --check` | 0 |
| Markdown lint | `npx --yes markdownlint-cli2@0.24.0 "**/*.md"` | 0 |
| Static analysis | `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 |
| Type checking | `cargo check --workspace --all-targets --locked` | 0 |
| Build | `cargo build --workspace --locked` | 0 |
| Tests | `cargo test --workspace --locked` | 0 |
| Docs site build | `uvx zensical==0.0.73 build` | 0 |

CI (`.github/workflows/ci.yml`) runs the code gates on Linux, macOS, and Windows on every push, and it is the only
place Kiri compiles for Windows: `aws-lc-rs` compiles C, which needs the Windows headers. It also runs the Markdown
lint, on Linux only, because the result does not depend on the system.

## Cross-user check

`cargo xtask cross-user` proves with a second real OS user that the daemon refuses it (NFR-SEC-02). It is a job in
CI and not a local gate, because it needs privileges: `sudo -n` to become `nobody` on Linux and macOS (run `sudo -v`
first), and an elevated shell on Windows, where it creates and removes a temporary account. Windows only runs it in CI.

## Serving the docs site

`uvx zensical==0.0.73 serve --open` serves it on `http://localhost:8000`. The VS Code task `docs: serve`
runs the same command.
