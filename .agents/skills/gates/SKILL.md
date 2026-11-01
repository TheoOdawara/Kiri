---
name: gates
description: The Kiri repository's gates, cheapest first, with their exact invocations, and how to serve the docs site. Use before calling any work done, in /tightship:closeout, and when building or serving the docs.
---

# gates

Run every gate from the repository root: Zensical reads `zensical.toml` only from the current folder.

| Gate | Invocation | Exit |
|---|---|---|
| Formatting | `cargo fmt --all --check` | 0 |
| Static analysis | `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 |
| Type checking | `cargo check --workspace --all-targets --locked` | 0 |
| Windows compile | `cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-msvc -- -D warnings` | 0 |
| Build | `cargo build --workspace --locked` | 0 |
| Tests | `cargo test --workspace --locked` | 0 |
| Docs site build | `uvx zensical==0.0.73 build` | 0 |

The Windows compile gate needs `rustup target add x86_64-pc-windows-msvc` once, and proves only that the
code compiles there. CI (`.github/workflows/ci.yml`) runs the code gates on Linux, macOS, and Windows on
every push, and it is the only place the Windows tests run.

## Serving the docs site

`uvx zensical==0.0.73 serve --open` serves it on `http://localhost:8000`. The VS Code task `docs: serve`
runs the same command.
