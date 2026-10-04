# kiri-tui

The interactive interface: inline in Milestone 1, full-screen later. It is an ACP client of the daemon and
depends on `kiri-core` for the protocol types only. Milestone: M1.

## What it never does

- It never runs the engine: no provider call, no tool, no file of the project.
- It never decides a permission; it shows the request and returns the user's answer.

## What it carries

| Concern | Requirements | Milestone |
| --- | --- | --- |
| Inline interface on the native scrollback | [FR-UI-01](../../requirements/functional/interfaces.md#fr-ui-01) | M1 |
| Seal, tagline, and palette | [FR-UI-06, 07](../../requirements/functional/interfaces.md#fr-ui-06) | M1 |
| Live input while streaming | [NFR-PERF-01](../../requirements/non-functional/performance.md#nfr-perf-01) | M1 |
| Truecolor fallback and `NO_COLOR` | [NFR-INTR-01, 02](../../requirements/non-functional/interaction.md) | M1 |
| Full-screen interface and the choice between the two | [FR-UI-02, 03](../../requirements/functional/interfaces.md#fr-ui-02) | Later |

Its inner blocks are not decided; the interface's spec decides them.
