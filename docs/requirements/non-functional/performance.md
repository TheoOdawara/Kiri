# Performance efficiency — `PERF`

ISO/IEC 25010 characteristic: performance efficiency.

<a id="nfr-perf-01"></a>
## NFR-PERF-01 — Live input

The system shall keep input live while the model streams or a tool runs.

| Attribute | Value |
| --- | --- |
| Rationale | The interface never blocks on the network or a tool. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-PERF-01.1** — While the model streams or a tool runs, a keystroke is drawn within 50 ms at the 95th percentile.


<a id="nfr-perf-02"></a>
## NFR-PERF-02 — Daemon idle exit

The system shall stop the daemon after an idle period with no running session and no connected client.

| Attribute | Value |
| --- | --- |
| Rationale | No process outlives its use. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-PERF-02.1** — Given no running session and no connected client, when 10 minutes pass, then the daemon has exited.
