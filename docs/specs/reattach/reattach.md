# #3 — Reattach to a session the terminal left

> **Status:** closed
> **Profile:** API
> **Module:** `crates/kiri-core` (`daemon`, `session`, `agent`, `protocol`), `crates/kiri-tui`, `crates/kiri`, `xtask`
> **Epic:** #3
> **Requirements:** FR-CNT-01.1, FR-CNT-01.2, FR-CNT-09, NFR-PERF-02. Constraints: NFR-FLEX-01, NFR-REL-01, NFR-REL-02.1, NFR-REL-03
> **Sprint:** 1

## Confirmed decisions

1. A connection that ends without `session/close` detaches its sessions. A running turn goes on, and what it sends is kept.
2. Ctrl-D closes the session for good. A detached session that never ran a prompt is discarded.
3. A session has at most one client. Only detached sessions are offered for reattach.
4. The project is the client's exact `cwd`. No detached session opens a new one, one is reattached at once, two or more are picked from a list.
5. Reattach replays the whole session through `session/load`, the running turn included, then continues live.
6. The daemon exits after 10 minutes with no connected client and no running turn. That replaces both exit rules of #2.
7. A version is the package version plus the executable's modification time, so a rebuild counts as another version.
8. A client that finds another version asks the daemon to exit, and the daemon refuses while a turn runs.
9. Replacing a daemon follows FR-CNT-09 to the letter. "Running session" is the glossary's: a turn in progress. Idle clients of other terminals lose their connection, and detached sessions with no running turn are lost.

## Acceptance Criteria

### Contract

JSON-RPC 2.0 in the types of `agent-client-protocol-schema` 1.13.0, protocol version 1, as in #2. This epic adds
these methods to #2's table, which stays in force:

| Method | Direction | Kind | Auth / Role | Idempotent |
| --- | --- | --- | --- | --- |
| `session/list` | client to daemon | request | Owner of the daemon | Yes |
| `session/load` | client to daemon | request | Owner of the daemon | No |
| `session/close` | client to daemon | request | Owner of the daemon | Yes |
| `_kiri/turn_ended` | daemon to client | notification | Owner of the daemon | No |
| `_kiri/exit` | client to daemon | request | Owner of the daemon | Yes |

`session/resume` and `session/delete` answer `-32601`, like any method outside the table. `initialize` now
advertises `agentCapabilities.loadSession: true` and `agentCapabilities.sessionCapabilities` with `list: {}` and
`close: {}`.

### Version identity

| Part | Value | Source |
| --- | --- | --- |
| Identity | `<package version>+<modification time>`, for example `0.1.0+1793628191` | Computed once, when the process starts |
| Package version | `CARGO_PKG_VERSION` of `kiri-core` | The binary |
| Modification time | Whole seconds since the Unix epoch of the running executable, or `unknown` when the OS cannot give it | `std::env::current_exe()` and its metadata |

The daemon answers its identity in `initialize`'s `agentInfo.version`. The client compares it to its own, byte for
byte.

### Request

`session/list`

| Field | Type | Required | Validation |
| --- | --- | --- | --- |
| `cwd` | string | No | Absolute path. Present: only sessions of that exact directory. Absent: every detached session. Comes from the client's working directory |
| `cursor` | string | No | Ignored; the list is never paged |

`session/load`

| Field | Type | Required | Validation |
| --- | --- | --- | --- |
| `sessionId` | string | Yes | A detached session of this daemon. Comes from `session/list` |
| `cwd` | string | Yes | Absolute; equal to the session's `cwd` |
| `mcpServers` | array | Yes | Accepted and ignored until the extensions milestone |
| `additionalDirectories` | array | No | Accepted and ignored |

`session/close`

| Field | Type | Required | Validation |
| --- | --- | --- | --- |
| `sessionId` | string | Yes | A session attached to this connection |

`_kiri/exit` takes empty params.

```json
{"jsonrpc":"2.0","id":2,"method":"session/list","params":{"cwd":"/home/theo/dev/kiri"}}
{"jsonrpc":"2.0","id":3,"method":"session/load","params":{"sessionId":"019a1f6e-...","cwd":"/home/theo/dev/kiri","mcpServers":[]}}
```

### Response

`session/list` answers the detached sessions, the most recently active first, with no `nextCursor`:

```json
{"jsonrpc":"2.0","id":2,"result":{"sessions":[{"sessionId":"019a1f6e-...","cwd":"/home/theo/dev/kiri","title":"Explain this error","_meta":{"kiri":{"running":true}}}]}}
```

| Field | Type | Source |
| --- | --- | --- |
| `sessionId` | string | The session's UUIDv7 |
| `cwd` | string | The `cwd` of its `session/new` |
| `title` | string | The first line of the session's first accepted prompt, trimmed, cut at 60 characters with `…` added when cut |
| `_meta.kiri.running` | boolean | `true` while a turn runs |

`updatedAt` is not sent. Activity is the start or the end of a turn, and it sets only the order of the list.

`session/load` attaches the session to the connection, sends the replay as notifications, then answers:

```json
{"jsonrpc":"2.0","id":3,"result":{"_meta":{"kiri":{"turnRunning":true}}}}
```

The replay is the session's transcript, in the order it was produced:

| Transcript entry | Sent as |
| --- | --- |
| An accepted prompt | `session/update`, `user_message_chunk`, with the prompt's text |
| Reasoning, text, a tool call, a tool call update | The same `session/update` the turn sent, batches as they were |
| The end of a turn | `_kiri/turn_ended` |

`_kiri/turn_ended` carries `sessionId` and either `stopReason`, with the values of #2's table, or `error` with
`code` and `message`:

```json
{"jsonrpc":"2.0","method":"_kiri/turn_ended","params":{"sessionId":"019a1f6e-...","stopReason":"end_turn"}}
{"jsonrpc":"2.0","method":"_kiri/turn_ended","params":{"sessionId":"019a1f6e-...","error":{"code":-32603,"message":"The endpoint failed (HTTP 503): overloaded"}}}
```

When `turnRunning` is `true`, the running turn's later updates and `_kiri/alive` reach this connection live, and its
end arrives as `_kiri/turn_ended`. A turn ends as the `session/prompt` answer only on the connection that sent its
prompt and is still attached.

`session/close` answers `{}`. `_kiri/exit` answers `{}` and then the daemon exits.

### Daemon invocation

| Argument | Type | Required | Validation |
| --- | --- | --- | --- |
| `--idle-exit-seconds` | integer | No | 1 to 86400; default 600. Hidden from `--help`. A client never passes it; tests and `xtask` do |

### Line client

`kiri` with no arguments, in `crates/kiri-tui`. Before the first prompt it calls `session/list` with its `cwd`.

| Detached sessions in the `cwd` | What the client does |
| --- | --- |
| 0 | `session/new`, as in #2 |
| 1 | `session/load` of that session, after printing "Reattached to: `<title>`" |
| 2 or more | Prints the list and reads a choice |

| Element | Text |
| --- | --- |
| List header | "Sessions in this project:" |
| List line | Two spaces, the number from 1, a period, a space, the title, a space, and `(running)` or `(idle)` |
| Choice prompt | "Pick a session by number, or press Enter for a new one: " |
| Choice that is neither a listed number nor empty | "Type a number from 1 to `<n>`, or press Enter for a new session." and the choice prompt again |
| Replayed prompt | `>`, a space, and the prompt's text, on its own line |
| Replayed turn output and ending | As #2's line client prints a live turn: text, `[reasoning]`, `[tool call]`, the notices, and `error: <message>` on stderr |
| Daemon of another version with a running turn | "The Kiri daemon runs version `<daemon identity>`, and this kiri is version `<client identity>`. A session is running, so the daemon was not replaced. Open Kiri again when it ends." on stderr |

Ctrl-D at the input prompt sends `session/close`, then ends the client with exit code 0. Ctrl-D at the choice prompt
ends the client with exit code 0 and attaches nothing. Ctrl-C and a closed terminal end the client the default way,
which detaches its session. After a reattach with `turnRunning` `true`, the client prints the turn live and shows
the input prompt when `_kiri/turn_ended` arrives.

### Roles and privileges

| Role | Permission | Note |
| --- | --- | --- |
| The OS user who owns the daemon | Every method | There is no other role. The transport enforces it, as in #2 |

---

## Business Rules

### 1. Authorization

Unchanged from #2's rule 1. Every new method sits behind the same owner-only transport, and a refused connection
counts as no client for rule 6.

### 2. Detach

`shortcut: a detached session lives only as long as the daemon, FR-CNT-03 replaces this with sessions on disk.`

A connection that ends for any reason other than `session/close` detaches each session attached to it. A detached
session that has never accepted a prompt is discarded. Every other one is kept, with its running turn going on
against the endpoint. A detached turn is never cancelled by the detach, and its updates enter the transcript and
reach no client. The log records "session detached" with the session ID.

### 3. Close

`session/close` cancels the session's running turn as `session/cancel` does, then discards the session. A closed
session is never listed again. A `session/close` for a session not attached to this connection answers `-32002` with
"Unknown session: `<id>`." The log records "session closed" with the session ID.

### 4. One client per session

`session/list` returns only detached sessions. `session/load` of a session attached to a connection answers
`-32602` with "This session is attached to another client." and changes nothing. A `session/prompt` or
`session/cancel` for a session not attached to the connection is handled as #2's rule 6 handles a session of
another connection.

### 5. Reattach and replay

`session/load` checks the session, then, as one step that no turn event can enter between, attaches it to the
connection and takes the transcript to replay. Every update the turn produces after that step goes live to the
connection. No update is lost or sent twice. The transcript holds every turn of the session since `session/new`,
including turns that failed or were cancelled. A later prompt on the reattached session carries the conversation, as
in #2's rule 6. The log records "session reattached" with the session ID, never the title.

`session/load` with a relative `cwd` answers `-32602` with "cwd must be an absolute path." A `cwd` that differs from
the session's answers `-32602` with "cwd does not match the session's directory." An ID that names no detached
session answers `-32002` with "Unknown session: `<id>`."

### 6. Idle exit

`shortcut: the idle period is fixed at 10 minutes, FR-PRV-09's configuration in #5 can make it a setting if anyone asks.`

The daemon exits with code 0 once 10 minutes pass with no connected client and no running turn. The clock starts
when the daemon starts, and restarts whenever both conditions become true again. A connection, or a turn that
starts, stops it. Detached sessions with no running turn do not hold the daemon, and they are lost when it exits.
The log records "daemon exited" with `reason` `idle`. #2's rule 4, which ended the daemon with its last client or 5
seconds after a start with no client, no longer applies. The invalid-environment path of #2's rule 5 is unchanged.

### 7. Version check

After `initialize`, the client compares the daemon's `agentInfo.version` with its own identity. Equal: the client
goes on to rule 8. Different: it sends `_kiri/exit`.

- The daemon answers `{}` when no turn runs in any session. It stops listening, ends every connection, removes the
  socket, releases its lock, and exits with code 0. The log records "daemon exited" with `reason` `replaced` and the
  client's identity from `initialize`'s `clientInfo`. The client then reaches a daemon as #2's rule 2 does, with a
  new 5 seconds, and goes on with the new one.
- The daemon answers `-32050` with "A session is running." when a turn runs, and touches nothing. The client prints
  the "another version" message of the line client table and exits with code 1.

The daemon decides, under the same lock that starts turns, so a turn that starts during the check either runs to
the refusal or is never started. Clients of other terminals see their connection end and print #2's "error: The
connection to the Kiri daemon was lost."

### 8. Choosing the session

The client lists the detached sessions of its `cwd` and follows the line client table. A number picks the session at
that position. An empty line opens a new session. A session that someone else loaded between the list and the
choice answers rule 4's error, and the client prints `error: <message>` and lists again.

### 9. Persistence and audit

- **Tables/columns changed:** none. Sessions, transcripts, and conversations live in memory.
- **Audit:** none. The log gains "session detached", "session reattached", "session closed", and "daemon exited"
  with its `reason` (`idle` or `replaced`). It never records a title, a prompt, a reply, or reasoning, as in #2.
- **Events/integrations fired:** the replay, `_kiri/turn_ended`, and the methods of the contract.

---

## Errors

| Code | When | Message |
| --- | --- | --- |
| `-32602` | `session/list` or `session/load` with a relative `cwd` | "cwd must be an absolute path." |
| `-32602` | `session/load` with a `cwd` other than the session's | "cwd does not match the session's directory." |
| `-32602` | `session/load` of a session attached to another connection | "This session is attached to another client." |
| `-32002` | `session/load` of an ID with no detached session, or `session/close` of a session not attached here | "Unknown session: `<id>`." |
| `-32050` | `_kiri/exit` while a turn runs | "A session is running." |

Failures with no JSON-RPC code, printed by the client:

| When | Message |
| --- | --- |
| The daemon is another version and a turn runs | "The Kiri daemon runs version `<daemon identity>`, and this kiri is version `<client identity>`. A session is running, so the daemon was not replaced. Open Kiri again when it ends." |
| A choice that is neither a listed number nor empty | "Type a number from 1 to `<n>`, or press Enter for a new session." |

## Side Effects

- **Persistence:** none on disk beyond #2's files. The session gains a transcript in memory, which grows with the
  session like its conversation.
- **Concurrency:** turns of detached sessions run alongside attached ones. Rule 5 makes load and live updates one
  ordered stream. Rule 7 makes the exit and the start of a turn mutually exclusive. Two clients loading the same
  session: the first wins, the second gets rule 4's error.
- **Transaction:** a turn enters the conversation whole or not at all, as in #2. A turn's transcript entries are
  appended as its updates are sent, so a reattach in mid-turn replays what was sent so far.

---

## Acceptance Scenarios (Gherkin)

All scenarios pass on Windows, Linux, and macOS (NFR-FLEX-01.1). Scenarios 5 and 6 of #2 are replaced by 11 and 12.

### Scenario 1 — Reattach to a running turn (happy path, FR-CNT-01.1)

```gherkin
Given a client in a project with a turn streaming its reply
When the client is killed
And the user runs kiri again in the same directory
Then the client prints "Reattached to: <title>"
And it prints "> " and the prompt, then the reply so far
And the rest of the reply arrives live
And the input prompt appears when the turn ends
```

### Scenario 2 — The turn ended while detached (alternative)

```gherkin
Given a turn whose client was killed
And the turn ended with end_turn while no client was attached
When the user runs kiri again in the same directory
Then the client prints the prompt and the whole reply
And shows the input prompt
And the next prompt's request holds the first prompt and the first reply
```

### Scenario 3 — The turn failed while detached (exception)

```gherkin
Given a turn whose client was killed
And the endpoint then answered HTTP 503 with the text "overloaded"
When the user runs kiri again in the same directory
Then the client prints "error: The endpoint failed (HTTP 503): overloaded" after the replayed prompt
And shows the input prompt
```

### Scenario 4 — Pick among two sessions (happy path, FR-CNT-01.2)

```gherkin
Given two clients in the same project, each with a session that ran a prompt
When both clients are killed
And the user runs kiri again in that directory
Then the client prints "Sessions in this project:" and two numbered lines, the most recently active first
And when the user types 2
Then the client reattaches to the second session listed and replays it
```

### Scenario 5 — Enter opens a new session (alternative)

```gherkin
Given two detached sessions in the project
When the user runs kiri there and presses Enter at the choice prompt
Then a new session opens with no replay
And both detached sessions are still listed by session/list
```

### Scenario 6 — An invalid choice (exception)

```gherkin
Given two detached sessions in the project
When the user types 3 at the choice prompt
Then the client prints "Type a number from 1 to 2, or press Enter for a new session."
And shows the choice prompt again
When the user presses Ctrl-D
Then the client exits with code 0 and both sessions stay detached
```

### Scenario 7 — Ctrl-D closes the session (alternative)

```gherkin
Given a client whose session ran a prompt
When the user presses Ctrl-D at the input prompt
And runs kiri again in the same directory
Then a new session opens with no list and no replay
```

### Scenario 8 — Sessions of another directory are not offered (alternative)

```gherkin
Given a detached session whose cwd is /a
When the user runs kiri in /b
Then a new session opens with no list
And session/list with no cwd still returns the session of /a
```

### Scenario 9 — A session has one client (exception)

```gherkin
Given a session attached to a connection
When a second connection calls session/list
Then the session is not in the answer
When the second connection calls session/load with its ID
Then it gets -32602 and "This session is attached to another client."
And the first connection keeps receiving the session's updates
```

### Scenario 10 — An empty session is not kept (alternative)

```gherkin
Given a client that opened a session and sent no prompt
When the client is killed
Then session/list returns no session for that directory
```

### Scenario 11 — Idle exit (NFR-PERF-02.1)

```gherkin
Given a daemon with no connected client and no running turn
When 10 minutes pass
Then the daemon has exited with code 0
And the log records "daemon exited" with reason idle
```

### Scenario 12 — A detached running turn keeps the daemon (alternative)

```gherkin
Given a daemon started with --idle-exit-seconds 2
And a detached session whose turn is still streaming
When 2 seconds pass
Then the daemon is still running
And 2 seconds after the turn ends, the daemon exits
```

### Scenario 13 — A connected client keeps the daemon (alternative)

```gherkin
Given a daemon started with --idle-exit-seconds 2
And a connected client with no running turn
When 4 seconds pass
Then the daemon is still running
And 2 seconds after the client disconnects, the daemon exits
```

### Scenario 14 — Replace a daemon of another version (happy path, FR-CNT-09.1)

```gherkin
Given a daemon whose executable's modification time differs from the client's
And no turn runs in it
When the user runs kiri
Then the old daemon exits with reason replaced in the log
And a daemon with the client's identity starts
And the session opens
```

### Scenario 15 — A running turn blocks the replacement (exception, FR-CNT-09.2)

```gherkin
Given a daemon of another version with a turn streaming in a client
When the user runs kiri in another terminal
Then that client prints "The Kiri daemon runs version <daemon identity>, and this kiri is version <client identity>. A session is running, so the daemon was not replaced. Open Kiri again when it ends."
And exits with code 1
And the running turn finishes and reaches its client
```

### Scenario 16 — Replacement ends idle clients of other terminals (alternative)

```gherkin
Given a daemon of another version with an idle client at its input prompt
When the user runs kiri in another terminal
Then the idle client prints "error: The connection to the Kiri daemon was lost." and exits with code 1
And the new client opens its session on a new daemon
```

### Scenario 17 — Protocol misuse (exception)

```gherkin
Given a detached session whose cwd is /a
When a client calls session/load with cwd /b
Then it gets -32602 and "cwd does not match the session's directory."
When it calls session/load with a relative cwd
Then it gets -32602 and "cwd must be an absolute path."
When it calls session/load with an unknown ID
Then it gets -32002 and "Unknown session: <id>."
When it calls session/close for a session it did not attach
Then it gets -32002 and "Unknown session: <id>."
When it calls session/resume
Then it gets -32601
```

### Scenario 18 — The exit request with a running turn (exception)

```gherkin
Given a turn running in a session
When a client sends _kiri/exit
Then it gets -32050 and "A session is running."
And the turn goes on
```

### Scenario 19 — No update lost or repeated across the reattach (alternative)

```gherkin
Given an endpoint that sends 1000 text deltas at 1 ms intervals
And the client is killed after the first 300
When a client loads the session while the stream goes on
Then the replayed text followed by the live text equals the endpoint's text exactly
```

### Scenario 20 — The log holds no content of a reattach (exception)

```gherkin
Given a session detached, listed, reattached, and closed
When the log is read
Then it holds "session detached", "session reattached", and "session closed" with the session ID
And it holds no title, prompt, reply, or reasoning
```

### Scenario 21 — A refused connection does not hold the daemon (denied authorization)

```gherkin
Given on Linux or macOS a daemon started with --idle-exit-seconds 5, with ~/.kiri and the socket made world-accessible
When a second real OS user connects and is refused
Then the daemon exits within 10 seconds of its start
```

### Scenario 22 — The picked session was taken in the meantime (exception)

```gherkin
Given two detached sessions in the project and a client showing the choice prompt
When another client loads the first session
And the user types 1
Then the client prints "error: This session is attached to another client."
And lists the sessions again, with only the second one
```

---

## Architecture impact

- **`kiri-core`:**
  - `session` owns every session for the whole daemon, no longer per connection, with its attachment, transcript,
    and title.
  - `daemon` holds the idle clock and `_kiri/exit`.
  - `agent` sends a turn's updates to whichever connection is attached, or to none, and appends them to the
    transcript.
  - `protocol::connect` returns the daemon's identity.
  - The `_kiri/*` surface gains `_kiri/turn_ended` and `_kiri/exit`.
- **`kiri`:** the client compares identities and replaces the daemon. `kiri daemon` gains the hidden
  `--idle-exit-seconds`, and exits after its idle period, never with its last client.
- **`kiri-tui`:** the line client lists, picks, loads, and prints a replay. Ctrl-D sends `session/close`.
- **`xtask`:** the cross-user check passes `--idle-exit-seconds` to the daemon it starts by hand and ends its
  daemons with `_kiri/exit`.
- **Runtime:**
  - "Reattaching to a running session" is rewritten with list, load, and replay, and settles what the client is
    shown.
  - "Starting the daemon" gains the version check and replaces its step 8 with the idle exit.
- **Concepts:** Streaming gains the transcript and the replay.
- **ADR 0002:** no new dependency.

## Data model impact

- **Session:** gains `title`, `transcript` (the ordered updates, prompts, and turn endings it sent), `attached`
  (the connection, or none when detached), and `last_activity` (the order of `session/list`). Held in memory; no
  file.
- Message and Provider: unchanged.
- Storage on disk gains no entity.

## Out of Scope

- A turn waiting for an approval when its terminal closes, FR-CNT-01.3: #8, which brings approvals.
- The interrupt key. Ctrl-C keeps detaching, as closing the terminal does: #6.
- Taking over a session attached to another terminal: no requirement asks for it.
- Sessions on disk, and keeping detached sessions past the daemon's exit: FR-CNT-03 is Won't for this release.
- Keeping idle clients and detached sessions through a replacement: FR-CNT-09 replaces a daemon with no running
  turn, and changing that is an SRS change.
- Daemons built before this epic: no release shipped them, and they lack `_kiri/exit`.
- On Windows, a rebuild fails while the daemon runs from the executable being replaced. The idle exit bounds it to
  10 minutes, and #12 reviews platform conformance.
- A configurable idle period: NFR-PERF-02 fixes 10 minutes.
- Paging `session/list`, `updatedAt`, `session/resume`, and `session/delete`: the line client needs none of them.
- The inline interface's version of the picker: #4.

## Task Breakdown

| # | Title | Scope | Acceptance criterion | Depends on |
| --- | --- | --- | --- | --- |
| 1 | Keep sessions alive across connections and exit when idle | `session` owns sessions daemon-wide; detach, `session/close`, Ctrl-D in the line client; idle clock and `--idle-exit-seconds`; `_kiri/exit` in the daemon; test teardown moves from "last client leaves" to `_kiri/exit` in `crates/kiri/tests/daemon/support.rs` and its 27 `wait_for_daemon_exit` calls, and in `xtask`'s `OwnerClient` and refusal check; #2's tests for rule 4 rewritten | Scenarios 7, 10, 11, 12, 13, 18, 21 | — |
| 2 | Reattach to a detached session with its replay | Transcript and title in `session`, `agent` sending to the attached connection, `session/list`, `session/load`, `_kiri/turn_ended`, `initialize` capabilities; line client listing, choice, replay printing | Scenarios 1, 2, 3, 4, 5, 6, 8, 9, 17, 19, 20, 22 | 1 |
| 3 | Replace a daemon of another version | Version identity, `agentInfo.version`, `protocol::connect` returning it, client comparison and `_kiri/exit` call, the "another version" message | Scenarios 14, 15, 16 | 1 |
