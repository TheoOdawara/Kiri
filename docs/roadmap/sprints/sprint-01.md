# Sprint 1 — 2026-11-02 → 2026-11-08

**Goal:** A user opens `kiri`, the daemon starts by itself, a prompt gets an answer streamed inline from an
OpenAI-compatible endpoint, and the user closes the terminal and reattaches to the session.
**Requirements baseline:** v3.0.0, no change requests to decide
**Items:** #2, #3, #4

## Specs first

None of the three epics has a spec. Each gets `/tightship:spec`, then `/tightship:epic`, before any code:

1. #2, Daemon and reattach
2. #3, Branded inline interface
3. #4, Streamed answers

The first code task creates the Cargo workspace and CI on Windows, Linux, and macOS.

## Scope

| Epic | In this sprint | Left for a later sprint |
| --- | --- | --- |
| #2 | FR-CNT-01.1, FR-CNT-01.2, FR-CNT-02, NFR-SEC-02 | FR-CNT-01.3, which needs approvals; FR-CNT-09; NFR-PERF-02 |
| #3 | FR-UI-01, FR-UI-06, NFR-PERF-01 | FR-UI-07, NFR-INTR-01, NFR-INTR-02 |
| #4 | FR-AGT-02, FR-PRV-01 | — |

The endpoint's URL and API key come from environment variables, read through the validation file. Setup
inside Kiri belongs to #5 (FR-PRV-09).
