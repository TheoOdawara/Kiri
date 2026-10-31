---
name: gates
description: The Kiri repository's gates, cheapest first, with their exact invocations, and how to serve the docs site. Use before calling any work done, in /tightship:closeout, and when building or serving the docs.
---

# gates

Run every gate from the repository root: Zensical reads `zensical.toml` only from the current folder.

| Gate | Invocation | Exit |
|---|---|---|
| Static analysis | n/a: there is no `Cargo.toml` yet | — |
| Type checking | n/a: there is no `Cargo.toml` yet | — |
| Formatting | n/a: there is no `Cargo.toml` yet | — |
| Build | n/a: there is no `Cargo.toml` yet | — |
| Tests | n/a: there is no `Cargo.toml` yet | — |
| Docs site build | `uvx zensical==0.0.73 build` | 0 |

When the workspace lands, each code gate gets its exact invocation and a validated exit code here.

## Serving the docs site

`uvx zensical==0.0.73 serve --open` serves it on `http://localhost:8000`. The VS Code task `docs: serve`
runs the same command.
