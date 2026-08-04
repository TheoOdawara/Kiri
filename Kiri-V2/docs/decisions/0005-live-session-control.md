# 0005. Live session control

- Status: accepted
- Date: 2026-08-04

## Context

The terminal should remain useful while an agent is thinking, running a
command, waiting for input, or supervising child agents. A blocking request
and response flow creates unnecessary interruptions and prevents the user
from inspecting, steering, or dispatching work in parallel.

## Decision

Kiri provides a live session control plane as a first-class part of the TUI.
The user can move between active sessions without stopping them.

The runtime is split into two local processes:

- The TUI is a client responsible for rendering and user input.
- The supervisor owns session state, input queues, child processes, and
  reconnection.

The supervisor starts on demand, communicates with the TUI through authenticated
local IPC, and exits when no sessions remain. It is a session runtime, not a
standing LLM orchestrator or an additional agent.

Multiple TUI clients can connect to the same supervisor. A client may observe
any session it is authorized to see, while each session has one controller
lease at a time. Other clients are read-only viewers until the controller
lease is explicitly transferred. Closing one client does not affect the
supervisor, other clients, or running sessions.

### Session dashboard

The dashboard presents every visible session with:

- repository and working directory;
- current mode and approval policy;
- current state and elapsed time;
- latest activity, tool, or question;
- child-agent summary;
- pending approvals and queued messages.

Sessions are grouped or filtered by state so sessions waiting for the user are
easy to find. Child agents roll up under the session that launched them while
remaining inspectable.

### Live interactions

While a session is active, the user can:

- inspect streamed output, diffs, tool activity, and audit events;
- answer questions and approvals inline;
- send a message immediately when the session is accepting input;
- queue a message while the session is busy;
- change mode or resource overrides for the next decision boundary;
- dispatch a new session or child agent without stopping existing work;
- request a cooperative pause;
- stop the current execution explicitly.

Queued input is delivered in order at the next safe turn boundary. A session
that is waiting for user input receives a response immediately.

### Concurrency

The supervisor enforces bounded concurrency:

- a global limit for active sessions;
- a per-project limit;
- a per-session limit for child agents;
- a hard safety ceiling that repository configuration cannot raise.

When a limit is reached, new work enters an explicit queue. The dashboard
shows each queued item, its position, and the limit that is blocking it.
Limits may be lowered by user or repository configuration within the protected
ceiling.

The default queue order is FIFO (first in, first out). The user can manually
reorder or cancel queued work from the dashboard. A queued session waiting for
user input does not consume an execution slot, and agents cannot change their
own priority.

### Pause and stop

`Pause` and `Stop` are separate actions:

- `Pause` is cooperative. Kiri lets the current tool action finish, then
  prevents the next agent action until resumed.
- `Stop` explicitly cancels the current turn and terminates the managed process
  tree according to the execution limits.

Neither action silently changes the plan or discards the session history.

### TUI exit policy

The user configures what happens to active sessions when the TUI closes:

- `detach`: sessions continue under the local supervisor.
- `pause`: sessions pause at the next safe boundary.
- `stop`: sessions are cancelled and their managed process trees are
  terminated.

A normal, intentional exit shows the active-session policy and lets the user
choose a different action for that exit. An unexpected crash or forced
termination cannot ask, so it applies the configured policy automatically. The
default for a new installation is `pause`.

Reopening Kiri reconnects to detached or paused sessions when their session
state remains available. The supervisor does not change the sandbox, limits,
credentials, or audit rules of a session.

### Session persistence

Session state is operational state, not memory and not project documentation.
Kiri persists enough state to reconnect to active, paused, waiting, completed,
failed, and interrupted sessions:

- Closing the TUI leaves detached sessions running.
- A supervisor restart reconnects when the managed processes still exist.
- A child-process crash marks the session failed while preserving its history.
- An operating-system restart marks active work interrupted; it cannot pretend
  that the original process continued.
- Completed sessions are inspectable and cannot resume execution without an
  explicit user action.

An interrupted session can be resumed only through the explicit command:

```text
kiri --resume [SESSION_ID]
```

Without `SESSION_ID`, Kiri opens a selector containing resumable sessions.
With an ID, it targets that session directly. Resume always performs state
reconciliation before starting a new agent turn.

### Runtime safety boundary

The live control plane does not weaken execution isolation. A process already
running keeps the sandbox envelope, credentials, network capabilities, and
limits captured at launch. A mode or resource change applies at the next
decision boundary and never expands an action retroactively.

The UI may request a stop, but it cannot inject new permissions into an
already-running process. A new action is evaluated again by the `PolicyEngine`.

## Consequences

- The user can supervise several sessions without repeatedly leaving the
  current workflow.
- Waiting for input becomes an explicit session state rather than a blocked
  terminal.
- Runtime control and execution authorization remain separate, preserving the
  security decisions in ADR 0003.
- Kiri needs durable session state, ordered input queues, cancellation, and
  event streaming, but does not need a second hidden orchestrator agent.

## Alternatives considered

- Blocking the TUI until each turn completes was rejected because it prevents
  inspection and parallel work.
- Applying new permissions to running processes was rejected because it would
  weaken the launch-time sandbox guarantee.
- Treating pause as process termination was rejected because users need a
  reversible cooperative control distinct from stopping work.

## References

- [Grok Build Agent Dashboard](https://x.ai/news/agent-dashboard)
- [Grok Build source repository](https://github.com/xai-org/grok-build)
