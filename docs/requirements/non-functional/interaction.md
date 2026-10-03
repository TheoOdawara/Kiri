# Interaction capability — `INTR`

ISO/IEC 25010 characteristic: interaction capability.

<a id="nfr-intr-01"></a>
## NFR-INTR-01 — Degradation without truecolor

The system shall stay fully readable on a terminal without truecolor.

| Attribute | Value |
| --- | --- |
| Rationale | Not every terminal has 24-bit color. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-INTR-01.1** — On a 256-color and on a 16-color terminal, 0 interface elements are distinguishable only by a truecolor value.


<a id="nfr-intr-02"></a>
## NFR-INTR-02 — NO_COLOR

The system shall honor the `NO_COLOR` environment variable.

| Attribute | Value |
| --- | --- |
| Rationale | The convention users already set. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-INTR-02.1** — Given `NO_COLOR` set, when any interface renders, then 0 color escape sequences are emitted.
