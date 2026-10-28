# Reattaching to a running session

A terminal closes while a turn runs; the engineer comes back to it. Milestone: M1.
Requirements: [FR-CNT-01, 02](../../requirements/functional/continuity.md#fr-cnt-01),
[NFR-PERF-02](../../requirements/non-functional/performance.md#nfr-perf-02).

1. The terminal closes; the **kiri** client process ends and its socket connection drops.
2. The **daemon** keeps the session: **agent** continues the turn with no client attached.
3. The engineer runs **kiri** again. It connects to the daemon's socket, or starts the daemon when none runs.
4. **session** attaches the new client to the running session.
5. The client receives the session's events from then on.
6. With no running session and no connected client, the daemon exits after its idle period.

What the client is shown of the events it missed is not decided; the daemon's spec decides it.
