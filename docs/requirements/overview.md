# Overview

## Stakeholders

| Stakeholder | Role | Need |
| --- | --- | --- |
| The author | Primary user and sole decision-maker | A terminal coding agent used daily, under full control |
| Developers installing the open-source release | Secondary users | Onboarding and documentation that work for someone who has never seen the repository ([OQ-06](open-questions.md)) |

## Operating environment

- A terminal on Windows, Linux, or macOS, each native.
- One daemon per operating-system user; every interface is a client of it ([ADR 0001](../decisions/0001-architecture.md)).
- State as text files under `~/.kiri` and `<project>/.kiri`.

## Constraints

- **Language:** Rust.
- **License:** MIT.
- **Architecture:** decided in [ADR 0001](../decisions/0001-architecture.md). One engine serves four front-ends, so the
  engine knows no interface; no layer, crate, or abstraction exists without a concrete consumer.
- **Persistence:** text only — JSONL, TOML, Markdown. No SQLite.
- **Process:** every capability has a closed spec before code.
- **Windows:** native. Running through WSL is not a supported path.
- **Previous implementation:** discarded, and not a reference for any decision.
- **Brand:** the one thing preserved, in [`docs/marca/`](../marca/).
  - Kept as they are: the logo, the block-character seal (`seal.txt`), and the tagline
    *Engineering-Grade Code Harness — Forged from Tradition, Built for Precision.*
  - Kept as a concept and redesigned: the forging-and-cooling motion.
  - A starting point: the "Tamahagane Void" palette. The redesign keeps the steel-and-gate character and reads
    lighter than the previous near-black.
  - Open for redesign: glyphs, screen layout, and the rest of the previous interface.

## Assumptions and dependencies

- Anthropic has blocked third-party OAuth use of Claude Pro/Max since 2026-04-04 and tolerates `claude -p`
  (verified 2026-09-30). FR-PRV-06 depends on the user's installed, logged-in `claude` CLI at a minimum version.
- OpenAI supports "Sign in with ChatGPT" for third-party tools (verified 2026-09-30). FR-PRV-05 depends on it.
- Sync depends on a private GitHub repository; `gh` is used when installed and is never required.
- Bundled skills depend on their upstream repositories staying available.
- IDE integration depends on the IDE implementing ACP.
