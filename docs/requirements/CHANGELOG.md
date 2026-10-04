# Changelog

## 1.0.1 — 2026-10-03

- **Changed:** the 73 requirements move from `proposed` to `approved`, confirmed by the author.
- **Changed:** the values of NFR-REL-02, NFR-PERF-01, and NFR-PERF-02 are confirmed; the pending note leaves
  their rationale.

## 1.0.0 — 2026-10-03

Kickoff of the versioned SRS, migrated from the single file `docs/requirements.md`.

- **Added:** the 73 requirements of the [index](README.md#requirements), all `proposed`.
- **Decided at kickoff:** the daemon and reattach are Milestone 1 (FR-CNT-01, FR-CNT-02); configurable permissions
  are Milestone 1 and are the only bound of auto mode until the sandbox ships (FR-SAF-03, FR-SAF-04); skills are
  fetched from their upstream independently of Kiri's releases (FR-EXT-08).
- **Added from ADR 0001:** plugins from git marketplaces (FR-EXT-09), subagent attach (FR-EXT-10),
  session-to-session messaging (FR-CNT-08), owner-only transport (NFR-SEC-02), Claude Code formats (NFR-COMP-01).
- **Proposed values awaiting confirmation:** NFR-REL-02, NFR-PERF-01, NFR-PERF-02.

## Before 1.0.0

The evolution log of `docs/requirements.md`, kept as written.

- **2026-09-30** — Kickoff after a full reset. The previous implementation is discarded; only the brand
  (assets, concept, palette, seal, tagline) is preserved. Scope captured from the author's interview:
  terminal coding agent, Rust, native on three platforms, open source, core-first milestones. Verified
  that Anthropic has blocked third-party use of Claude Pro/Max OAuth since 2026-04-04 and that OpenAI
  officially supports "Sign in with ChatGPT" for third-party tools; impersonating a vendor client moved to
  Out of scope.
- **2026-09-30** — Claude Pro/Max subscription moved back into scope by the author's decision, as an
  opt-in with an explicit ban-risk warning; the mechanism became an open question. Anthropic-compatible
  endpoints (MiniMax, GLM, Kimi) added as a provider kind.
- **2026-09-30** — Author settled five open questions: Milestone 1 ships the inline interface; Claude
  Pro/Max goes through Kiri's own proxy; ChatGPT uses the same OAuth flow other agents offer; Windows gets
  a native sandbox; wave order is decided as development proceeds. Added: bundled `ponytail` and
  `humanize` skills that track their upstream; a free, low-end-hardware classifier as a constraint; sync
  data lives in `~/.kiri`.
- **2026-09-30** — Bundled skills set to `ponytail`, `humanizer`, `i-have-adhd`, `grill-me`, with their
  upstream repos. Jev recorded as the classifier's reference behavior; it stays out as the implementation
  because it is paid and remote.
- **2026-09-30** — Classifier made optional and user-chosen (any configured model); without one, auto
  mode rests on the sandbox, configurable permissions, and the user's workflow. The free/local-hardware
  constraint was dropped. Sync goes through a private GitHub repo offered at first run (via `gh`),
  declinable. `grill-me` upstream set to mattpocock/skills.
- **2026-09-30** — Architecture decided in ADR 0001 (daemon owning every session, ACP boundary, three
  crates, capability modules, data-only extensions). Claude Pro/Max moved from an own proxy to driving the
  official `claude` CLI, after research showed impersonation breaks on every Claude Code release while
  `claude -p` remains the tolerated path; the warning was reworded accordingly. Sessions now survive a
  closed terminal.
- **2026-09-30** — Brand scope narrowed by the author: logo, block-character seal, and tagline are kept
  as they are; the forging/cooling motion is kept as a concept to be redesigned; the palette is a starting
  point to be made lighter and more readable; glyphs and layout are open.
