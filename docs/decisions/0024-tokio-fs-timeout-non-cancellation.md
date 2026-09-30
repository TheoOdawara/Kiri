# ADR 0024 — `tokio::fs` timeouts can't cancel the underlying syscall (accepted limitation)

- Status: Accepted
- Date: 2026-07-06
- Relates to: the fs tool timeouts added for issue #9 (`write_file`, `edit_file`, `delete_file`,
  `delete_dir`, `move_path`, `create_dir`, `list_dir`, `search`)

## Context

Every fs tool's blocking I/O is wrapped in `tokio::time::timeout(exec::DEFAULT_TIMEOUT, tokio::fs::...)`
(issue #9's fix — this project's "all I/O has a timeout" non-negotiable). Audited as issue #53:
`tokio::time::timeout` only stops *awaiting* the wrapped future. `tokio::fs` operations run on tokio's
blocking thread pool (`spawn_blocking` under the hood) — a real OS thread executing the real syscall
(`write`, `rename`, `remove_file`, `remove_dir_all`, …). Dropping the future when the timeout fires does
not, and cannot, interrupt that thread: there is no cross-platform way to cancel a blocking syscall already
handed to the kernel. The syscall runs to completion (or its own OS-level failure) regardless of what the
async caller decided to do with the timed-out future.

Practical consequence: a `write_file` call that times out and reports an error may still have its write
land on disk moments later, after the harness (and the model) have already moved on believing it failed.
Every mutating fs tool's timeout branch (`write_file`, `edit_file`, `delete_file`, `delete_dir`,
`move_path`, `create_dir`) now reports this honestly — "…timed out (it may still complete in the
background)" — rather than implying a clean stop. `edit_file`'s write-timeout branch was missing this
disclaimer until this ADR's review surfaced the inconsistency (it had only "timed out", no background-
completion caveat); brought in line with the other five as part of closing this issue.

### Alternatives considered

- **Cancel the syscall directly.** Not possible in safe, cross-platform Rust — there is no portable
  "abort this thread's blocking syscall" primitive; the closest platform-specific mechanisms (POSIX signals
  interrupting a blocked FD, Windows `CancelSynchronousIo`) would need per-OS `unsafe` FFI, which this
  crate's `unsafe_code = "forbid"` lint and no-FFI-for-filesystem-ops convention (ADR 0009: OS confinement
  uses only system *binaries*, `sandbox-exec`/`bwrap`, never raw syscalls) both rule out.
- **A dedicated, bounded thread pool for fs ops, killed and rebuilt on a timeout.** Considered and
  **rejected**: this addresses a *different*, secondary risk (a hung blocking-pool thread eventually
  exhausting tokio's shared blocking pool, since `spawn_blocking` recycles a bounded number of threads and
  a permanently-stuck one is one fewer available to every other blocking task in the process) — not the
  primary risk this ADR is about, which is the syscall's *effect* still landing after the timeout is
  reported. Rebuilding a pool doesn't stop the original thread's syscall either; it only isolates *future*
  blocking work from a *degraded* pool. Adding a whole second thread-pool abstraction to solve a secondary
  risk, while leaving the primary one (a late-landing write) exactly as present, is not proportionate here.

## Decision

**Accept the limitation as inherent to `tokio::fs`/`spawn_blocking`, not fixable by contained application
code (Option A).** No further code change closes the non-cancellation gap itself. Two things *are* required
and already hold:

1. **Honest reporting.** Every mutating fs tool's timeout error message says the operation may still land
   in the background — already shipped with issue #9's fix, unchanged by this ADR.
2. **No auto-retry.** The harness must never automatically re-issue a timed-out mutating call: since the
   original operation may complete *after* the timeout is reported, a blind retry risks a second write/move
   /delete racing the still-in-flight first one, landing in an unpredictable order. Audited: `AgentLoop::run`
   has no tool-specific retry logic anywhere — a `ToolOutcome::Error` (timeout or otherwise) becomes exactly
   one `tool_result` message, and the loop always waits for the model's own next turn before doing anything
   else with that tool. Re-attempting a call is entirely the model's decision, informed by the honest error
   message, never the harness's.

## Consequences

- No code change from this ADR alone — it documents and closes issue #53's residual risk as an accepted,
  inherent limitation rather than a bug to keep chasing.
- Locked by `a_tool_timeout_is_never_auto_retried_within_the_turn` (`agent_loop/tests.rs`): a scripted turn
  whose single tool call times out (via `run_command`'s real, exercisable timeout — the same "the harness
  never re-issues a timed-out call by itself" behavior every tool relies on) produces exactly one
  `tool_result` and one `tool_finished` observation; the turn then simply waits for the next model turn.
- If a future change ever introduces automatic tool-call retry (e.g. for network resilience), it must
  special-case tokio::fs-backed mutating tools — retrying `write_file`/`move_path`/`delete_file`/
  `delete_dir`/`create_dir` after a timeout is unsafe by construction per this ADR, not merely untested.
- Future direction, if ever prioritized: a real fix would need an OS-level cancellable I/O primitive
  (io_uring on Linux, IOCP on Windows, kqueue on macOS) behind a new abstraction — a materially larger
  change than this remediation's scope, and not undertaken here.
