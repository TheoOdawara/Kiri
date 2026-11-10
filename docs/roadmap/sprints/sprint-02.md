# Sprint 2 — 2026-11-09 → 2026-11-15

**Goal:** A user closes the terminal during a turn, opens `kiri` again, and finds the session in the branded inline
interface; the daemon exits after 10 idle minutes.
**Requirements baseline:** v3.1.0. No change request open.
**Items:** #3 (#21, #22, #23), #4
**Carried over:** #3 and #4, from Sprint 1

## Order

1. #21, Keep sessions alive across connections and exit when idle
2. #22, Reattach to a detached session with its replay
3. #23, Replace a daemon of another version
4. #4's spec, `/tightship:spec` then `/tightship:epic`, before any of its code
5. #4, Branded inline interface

## Scope

| Epic | In this sprint | Left for a later sprint |
| --- | --- | --- |
| #3 | FR-CNT-01.1, FR-CNT-01.2, FR-CNT-09, NFR-PERF-02 | — |
| #4 | FR-UI-01, FR-UI-06, NFR-PERF-01 | FR-UI-07, NFR-INTR-01, NFR-INTR-02 |
