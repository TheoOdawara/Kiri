# Kiri — Software Requirements Specification

**Version:** 1.0.1 · **Date:** 2026-10-03 · [Changelog](CHANGELOG.md)

## Purpose

AI coding agents optimize for speed over control: edits scroll past, "done" is declared on red builds, and
each tool locks the user to one vendor. Kiri is a terminal coding agent for engineers who want the opposite:

- **Full control** — every reasoning step, tool call, and diff is visible; nothing with side effects runs
  without the user's policy allowing it.
- **Any model** — the user brings whatever provider they have: an API key, a local model, or a subscription.
- **Built-in gate** — the agent cannot declare work finished while the repository's own checks are red.

## Scope

**In scope.** Milestone 1 (`M1`) is the core, usable daily: the agent loop, providers by API key and local
endpoint, the built-in tools, approval modes with configurable permissions, the inline interface on the
daemon, and the brand. Everything else is `Later`; its order is decided as development proceeds.

**Out of scope.**

| Item | Reason |
| --- | --- |
| Running Windows through WSL | Windows is native |
| GUI, desktop app, or web UI | Kiri is a terminal agent |
| Impersonating a vendor's client to use a subscription | It breaks on every vendor release ([ADR 0001](../decisions/0001-architecture.md)) |
| Hosted or cloud agent execution | [OQ-08](open-questions.md) |
| Team or multi-user features, accounts, billing | [OQ-08](open-questions.md) |

## Contents

[Overview](overview.md) · [Glossary](glossary.md) · [Functional](functional/README.md) ·
[Non-functional](non-functional/README.md) · [Business rules](business-rules.md) ·
[Open questions](open-questions.md)

## Requirements

| ID | Name | Priority | Status | Milestone |
| --- | --- | --- | --- | --- |
| [FR-AGT-01](functional/agent.md#fr-agt-01) | Multi-turn loop | Must | approved | M1 |
| [FR-AGT-02](functional/agent.md#fr-agt-02) | Live streaming | Must | approved | M1 |
| [FR-AGT-03](functional/agent.md#fr-agt-03) | Turn interruption | Must | approved | M1 |
| [FR-AGT-04](functional/agent.md#fr-agt-04) | Project instructions | Must | approved | M1 |
| [FR-PRV-01](functional/providers.md#fr-prv-01) | OpenAI-compatible endpoint | Must | approved | M1 |
| [FR-PRV-02](functional/providers.md#fr-prv-02) | Anthropic and Anthropic-compatible endpoint | Must | approved | M1 |
| [FR-PRV-03](functional/providers.md#fr-prv-03) | OpenAI | Must | approved | M1 |
| [FR-PRV-04](functional/providers.md#fr-prv-04) | Local model server | Must | approved | M1 |
| [FR-PRV-05](functional/providers.md#fr-prv-05) | ChatGPT subscription | Won't (this release) | approved | Later |
| [FR-PRV-06](functional/providers.md#fr-prv-06) | Claude subscription | Won't (this release) | approved | Later |
| [FR-PRV-07](functional/providers.md#fr-prv-07) | Claude subscription opt-in warning | Won't (this release) | approved | Later |
| [FR-PRV-08](functional/providers.md#fr-prv-08) | Live switching | Must | approved | M1 |
| [FR-PRV-09](functional/providers.md#fr-prv-09) | In-app provider setup | Must | approved | M1 |
| [FR-TOL-01](functional/tools.md#fr-tol-01) | Read files | Must | approved | M1 |
| [FR-TOL-02](functional/tools.md#fr-tol-02) | Search files | Must | approved | M1 |
| [FR-TOL-03](functional/tools.md#fr-tol-03) | Edit files | Must | approved | M1 |
| [FR-TOL-04](functional/tools.md#fr-tol-04) | Write files | Must | approved | M1 |
| [FR-TOL-05](functional/tools.md#fr-tol-05) | Run shell commands | Must | approved | M1 |
| [FR-TOL-06](functional/tools.md#fr-tol-06) | Edits shown as diffs | Must | approved | M1 |
| [FR-SAF-01](functional/safety.md#fr-saf-01) | Plan mode | Must | approved | M1 |
| [FR-SAF-02](functional/safety.md#fr-saf-02) | Default mode | Must | approved | M1 |
| [FR-SAF-03](functional/safety.md#fr-saf-03) | Auto mode | Must | approved | M1 |
| [FR-SAF-04](functional/safety.md#fr-saf-04) | Configurable permissions | Must | approved | M1 |
| [FR-SAF-05](functional/safety.md#fr-saf-05) | Auto-mode classifier | Won't (this release) | approved | Later |
| [FR-SAF-06](functional/safety.md#fr-saf-06) | Classifier model choice | Won't (this release) | approved | Later |
| [FR-SAF-07](functional/safety.md#fr-saf-07) | OS confinement | Won't (this release) | approved | Later |
| [FR-SAF-08](functional/safety.md#fr-saf-08) | Trust approval | Must | approved | M1 |
| [FR-GAT-01](functional/gate.md#fr-gat-01) | Check discovery | Won't (this release) | approved | Later |
| [FR-GAT-02](functional/gate.md#fr-gat-02) | Gate before finish | Won't (this release) | approved | Later |
| [FR-GAT-03](functional/gate.md#fr-gat-03) | Attempt budget | Won't (this release) | approved | Later |
| [FR-UI-01](functional/interfaces.md#fr-ui-01) | Inline interface | Must | approved | M1 |
| [FR-UI-02](functional/interfaces.md#fr-ui-02) | Full-screen interface | Won't (this release) | approved | Later |
| [FR-UI-03](functional/interfaces.md#fr-ui-03) | Interface choice | Won't (this release) | approved | Later |
| [FR-UI-04](functional/interfaces.md#fr-ui-04) | Headless mode | Won't (this release) | approved | Later |
| [FR-UI-05](functional/interfaces.md#fr-ui-05) | IDE integration | Won't (this release) | approved | Later |
| [FR-UI-06](functional/interfaces.md#fr-ui-06) | Seal on launch | Must | approved | M1 |
| [FR-UI-07](functional/interfaces.md#fr-ui-07) | Brand palette | Must | approved | M1 |
| [FR-EXT-01](functional/extensions.md#fr-ext-01) | MCP servers | Won't (this release) | approved | Later |
| [FR-EXT-02](functional/extensions.md#fr-ext-02) | Skills | Won't (this release) | approved | Later |
| [FR-EXT-03](functional/extensions.md#fr-ext-03) | Commands | Won't (this release) | approved | Later |
| [FR-EXT-04](functional/extensions.md#fr-ext-04) | Hooks | Won't (this release) | approved | Later |
| [FR-EXT-05](functional/extensions.md#fr-ext-05) | Subagents | Won't (this release) | approved | Later |
| [FR-EXT-06](functional/extensions.md#fr-ext-06) | Global and project levels | Won't (this release) | approved | Later |
| [FR-EXT-07](functional/extensions.md#fr-ext-07) | Bundled skills | Won't (this release) | approved | Later |
| [FR-EXT-08](functional/extensions.md#fr-ext-08) | Skill updates from upstream | Won't (this release) | approved | Later |
| [FR-EXT-09](functional/extensions.md#fr-ext-09) | Plugins from git marketplaces | Won't (this release) | approved | Later |
| [FR-EXT-10](functional/extensions.md#fr-ext-10) | Subagent attach | Won't (this release) | approved | Later |
| [FR-CNT-01](functional/continuity.md#fr-cnt-01) | Sessions survive the terminal | Must | approved | M1 |
| [FR-CNT-02](functional/continuity.md#fr-cnt-02) | Daemon on demand | Must | approved | M1 |
| [FR-CNT-03](functional/continuity.md#fr-cnt-03) | List and resume sessions | Won't (this release) | approved | Later |
| [FR-CNT-04](functional/continuity.md#fr-cnt-04) | Checkpoints and rewind | Won't (this release) | approved | Later |
| [FR-CNT-05](functional/continuity.md#fr-cnt-05) | Persistent memory | Won't (this release) | approved | Later |
| [FR-CNT-06](functional/continuity.md#fr-cnt-06) | Sync across machines | Won't (this release) | approved | Later |
| [FR-CNT-07](functional/continuity.md#fr-cnt-07) | Sync setup offer | Won't (this release) | approved | Later |
| [FR-CNT-08](functional/continuity.md#fr-cnt-08) | Session-to-session messaging | Won't (this release) | approved | Later |
| [FR-DST-01](functional/distribution.md#fr-dst-01) | Cargo install | Won't (this release) | approved | Later |
| [FR-DST-02](functional/distribution.md#fr-dst-02) | Prebuilt binaries | Won't (this release) | approved | Later |
| [FR-DST-03](functional/distribution.md#fr-dst-03) | Install script | Won't (this release) | approved | Later |
| [FR-DST-04](functional/distribution.md#fr-dst-04) | Package managers | Won't (this release) | approved | Later |
| [FR-DST-05](functional/distribution.md#fr-dst-05) | Auto-update | Won't (this release) | approved | Later |
| [NFR-FLEX-01](non-functional/flexibility.md#nfr-flex-01) | Platform parity | Must | approved | M1 |
| [NFR-REL-01](non-functional/reliability.md#nfr-rel-01) | No crash on a runtime path | Must | approved | M1 |
| [NFR-REL-02](non-functional/reliability.md#nfr-rel-02) | Bounded calls | Must | approved | M1 |
| [NFR-REL-03](non-functional/reliability.md#nfr-rel-03) | Failures reported with a cause | Must | approved | M1 |
| [NFR-PERF-01](non-functional/performance.md#nfr-perf-01) | Live input | Must | approved | M1 |
| [NFR-PERF-02](non-functional/performance.md#nfr-perf-02) | Daemon idle exit | Must | approved | M1 |
| [NFR-SEC-01](non-functional/security.md#nfr-sec-01) | Secrets stay out of files and logs | Must | approved | M1 |
| [NFR-SEC-02](non-functional/security.md#nfr-sec-02) | Owner-only transport | Must | approved | M1 |
| [NFR-INTR-01](non-functional/interaction.md#nfr-intr-01) | Degradation without truecolor | Must | approved | M1 |
| [NFR-INTR-02](non-functional/interaction.md#nfr-intr-02) | NO_COLOR | Must | approved | M1 |
| [NFR-COMP-01](non-functional/compatibility.md#nfr-comp-01) | Claude Code extension formats | Won't (this release) | approved | Later |
| [BR-01](business-rules.md#br-01) | Tool output is data | Must | approved | M1 |
| [BR-02](business-rules.md#br-02) | Untrusted sources never loosen safety | Must | approved | M1 |
