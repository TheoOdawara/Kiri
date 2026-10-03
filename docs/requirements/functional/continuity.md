# Continuity — `CNT`

What outlives a terminal, a session, and a machine.

<a id="fr-cnt-01"></a>
## FR-CNT-01 — Sessions survive the terminal

> As an engineer, I want a running session to outlive my terminal, so that closing a window never kills work in progress.

The system shall keep a running session alive when its terminal closes and let the user reattach to it.

| Attribute | Value |
| --- | --- |
| Rationale | The daemon owns every session. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-01.1** — Given a running session, when the user closes the terminal and opens Kiri again, then the session is still running and the user reattaches to it.


<a id="fr-cnt-02"></a>
## FR-CNT-02 — Daemon on demand

> As an engineer, I want the daemon started for me, so that I never manage a background process.

The system shall start the daemon when the first client needs it, with no action from the user.

| Attribute | Value |
| --- | --- |
| Rationale | One daemon per user, started by the first client. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-02.1** — Given no daemon running, when the user opens Kiri, then a session starts with no separate command.


<a id="fr-cnt-03"></a>
## FR-CNT-03 — List and resume sessions

> As an engineer, I want to list and resume past conversations of a project, so that I continue yesterday's work.

The system shall list a project's past sessions and resume the one the user picks.

| Attribute | Value |
| --- | --- |
| Rationale | Work spans days. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-03.1** — Given a past session, when the user resumes it, then the conversation continues where it stopped.


<a id="fr-cnt-04"></a>
## FR-CNT-04 — Checkpoints and rewind

> As an engineer, I want to roll the agent's file edits back, so that a wrong direction costs nothing.

The system shall roll the agent's file edits back to a point of the conversation the user picks.

| Attribute | Value |
| --- | --- |
| Rationale | Control includes undo. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-04.1** — Given agent edits made after a checkpoint, when the user rewinds to it, then the files return to their content at that point.


<a id="fr-cnt-05"></a>
## FR-CNT-05 — Persistent memory

> As an engineer, I want the agent to keep facts and preferences, so that I do not repeat myself in every session.

The system shall keep facts and preferences across sessions and make them available to the agent.

| Attribute | Value |
| --- | --- |
| Rationale | A session starts from what earlier ones learned. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-05.1** — Given a fact stored in memory, when a later session needs it, then the agent recalls it.


<a id="fr-cnt-06"></a>
## FR-CNT-06 — Sync across machines

> As an engineer, I want my configuration, memory, and extensions on every machine, so that each machine behaves the same.

The system shall sync configuration, memory, and extensions under `~/.kiri` across the user's machines through a private GitHub repository.

| Attribute | Value |
| --- | --- |
| Rationale | The user works on more than one machine. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-06.1** — Given two machines with sync on, when memory or extensions change on one, then the other receives the change.


<a id="fr-cnt-07"></a>
## FR-CNT-07 — Sync setup offer

> As an engineer, I want sync offered at first run, so that I turn it on in one step or skip it.

The system shall offer, at first run, to create the sync repository, using `gh` when it is installed, and work fully when the user declines.

| Attribute | Value |
| --- | --- |
| Rationale | Sync is optional. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-07.1** — Given a first run, when the user declines creating the sync repository, then Kiri works normally.
- **FR-CNT-07.2** — Given sync declined at first run, when the user enables it later, then sync starts.


<a id="fr-cnt-08"></a>
## FR-CNT-08 — Session-to-session messaging

> As an engineer, I want sessions to message each other, so that parallel work coordinates.

The system shall let a session send a message to another session of the same user.

| Attribute | Value |
| --- | --- |
| Rationale | Wanted in the base of the architecture. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-CNT-08.1** — Given two running sessions, when one sends a message to the other, then the other receives it.
