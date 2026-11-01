# Reliability — `REL`

ISO/IEC 25010 characteristic: reliability.

## NFR-REL-01 — No crash on a runtime path {#nfr-rel-01}

The system shall keep the daemon and every client running when a network, process, or file operation fails.

| Attribute | Value |
| --- | --- |
| Rationale | Instability sank the previous codebase. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria:**

- **NFR-REL-01.1** — Given a failing network, process, or file operation during a session, when it occurs, then 0 Kiri processes terminate abnormally.

## NFR-REL-02 — Bounded calls {#nfr-rel-02}

The system shall bound every network request and every request to a child process by a timeout.

| Attribute | Value |
| --- | --- |
| Rationale | A call that never returns is a hang. A local model can spend more than 60 seconds on a long prompt before its first byte, so the network value is per provider. |
| Source | Author interview, 2026-09-30; review 2026-10-26 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria:**

- **NFR-REL-02.1** — A network request that receives no bytes for its provider's idle timeout fails with a timeout; the default is 60 seconds, and the user can set another per provider.
- **NFR-REL-02.2** — A shell command ends at its timeout, 120 seconds unless the model or the user sets another.
- **NFR-REL-02.3** — 0 network or child-process requests wait without a timeout. A long-lived child process (an MCP server, the `claude` CLI) is bounded per request, not over its life.

## NFR-REL-03 — Failures reported with a cause {#nfr-rel-03}

The system shall report every failure to the user with its cause.

| Attribute | Value |
| --- | --- |
| Rationale | Silence is the worst failure. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria:**

- **NFR-REL-03.1** — Given any failed operation, when it fails, then the user sees a message naming the cause; 0 failures are silent.
