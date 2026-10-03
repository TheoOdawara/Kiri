# Reliability — `REL`

ISO/IEC 25010 characteristic: reliability.

<a id="nfr-rel-01"></a>
## NFR-REL-01 — No crash on a runtime path

The system shall keep the daemon and every client running when a network, process, or file operation fails.

| Attribute | Value |
| --- | --- |
| Rationale | Instability sank the previous codebase. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-REL-01.1** — Given a failing network, process, or file operation during a session, when it occurs, then 0 Kiri processes terminate abnormally.


<a id="nfr-rel-02"></a>
## NFR-REL-02 — Bounded calls

The system shall bound every network call and every child-process call by a timeout.

| Attribute | Value |
| --- | --- |
| Rationale | A call that never returns is a hang. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-REL-02.1** — A network call that receives no bytes for 60 seconds fails with a timeout.
- **NFR-REL-02.2** — A child-process call ends at its timeout, 120 seconds unless the caller sets another.
- **NFR-REL-02.3** — 0 network or process calls wait without a timeout.


<a id="nfr-rel-03"></a>
## NFR-REL-03 — Failures reported with a cause

The system shall report every failure to the user with its cause.

| Attribute | Value |
| --- | --- |
| Rationale | Silence is the worst failure. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-REL-03.1** — Given any failed operation, when it fails, then the user sees a message naming the cause; 0 failures are silent.
