# Flexibility — `FLEX`

ISO/IEC 25010 characteristic: flexibility.

<a id="nfr-flex-01"></a>
## NFR-FLEX-01 — Platform parity

The system shall run natively on Windows, Linux, and macOS with the same feature set and no emulation layer.

| Attribute | Value |
| --- | --- |
| Rationale | All three platforms are first-class. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-FLEX-01.1** — 100 % of the acceptance criteria of each delivered requirement pass on Windows, on Linux, and on macOS.
- **NFR-FLEX-01.2** — 0 features depend on WSL, a virtual machine, or another emulation layer.
