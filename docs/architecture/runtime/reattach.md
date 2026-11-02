# Reattaching to a running session

A terminal closes while a turn runs; the engineer comes back to it. Milestone: M1.
Requirements: [FR-CNT-01, 02](../../requirements/functional/continuity.md#fr-cnt-01),
[NFR-PERF-02](../../requirements/non-functional/performance.md#nfr-perf-02).
Spec: [#3](../../specs/reattach/reattach.md).

1. The terminal closes; the **kiri** client process ends and its socket connection drops without `session/close`.
2. **session** detaches the connection's sessions. A session that never ran a prompt is discarded. **agent**
   continues the turn with no client attached and appends its updates to the session's transcript.
3. The engineer runs **kiri** again. It connects to the daemon's socket, or starts the daemon when none runs, and
   checks the daemon's version ([Starting the daemon](starting-the-daemon.md)).
4. **kiri** calls `session/list` with its working directory. With no detached session it opens a new one; with one
   it loads it; with two or more the engineer picks one, or a new session.
5. `session/load` attaches the session to the new connection and, in the same step, takes its transcript. The
   daemon replays the transcript: each prompt, every update the turns sent, and each turn's end as
   `_kiri/turn_ended`.
6. The client receives the running turn's later updates live, with no update lost or repeated, and the turn's end
   as `_kiri/turn_ended`.
7. With no connected client and no running turn for 10 minutes, the daemon exits. Detached sessions are lost with
   it until [FR-CNT-03](../../requirements/functional/continuity.md#fr-cnt-03) ships.

Ctrl-D closes the session with `session/close` instead of detaching it. A session has one client at a time.
