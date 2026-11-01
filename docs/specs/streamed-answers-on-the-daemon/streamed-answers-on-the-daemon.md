# #2 — Stream answers through the daemon

> **Status:** published
> **Profile:** API
> **Module:** `crates/kiri-core` (`protocol`, `session`, `agent`, `provider`), `crates/kiri-tui`, `crates/kiri`, `xtask`
> **Epic:** #2
> **Requirements:** FR-CNT-02, NFR-SEC-02, FR-AGT-02, FR-PRV-01, FR-PRV-10. Constraints: NFR-FLEX-01, NFR-REL-01, NFR-REL-02.1, NFR-REL-02.3, NFR-REL-03
> **Sprint:** 1

## Confirmed decisions

1. The daemon reads the environment once, at start, through one validation file. Base URL, API key, and model are required.
2. Until #3, a session lives and dies with its connection, and the daemon exits when its last client leaves.
3. Several clients run at once, each with its own session. The conversation lives in memory only.
4. The provider speaks streamed Chat Completions. Reasoning, text, and tool calls all cross the pipeline, and no tool runs.
5. The protocol is stable ACP v1, on our own JSON-RPC layer, as one JSON message per line.
6. NFR-SEC-02.1 is proved with a second real OS user on all three systems, in CI.
7. The best-known models get native reasoning settings from a catalog in the binary. Every other model uses `KIRI_REQUEST_PARAMS`, which also overrides the catalog.

## Acceptance Criteria

### Contract

JSON-RPC 2.0 messages in the types of `agent-client-protocol-schema` 1.13.0, protocol version 1, without the `unstable_protocol_v2` feature.

| Method | Direction | Kind | Auth / Role | Idempotent |
| --- | --- | --- | --- | --- |
| `initialize` | client to daemon | request | Owner of the daemon | Yes |
| `session/new` | client to daemon | request | Owner of the daemon | No |
| `session/prompt` | client to daemon | request | Owner of the daemon | No |
| `session/cancel` | client to daemon | notification | Owner of the daemon | Yes |
| `session/update` | daemon to client | notification | Owner of the daemon | No |

Any other method answers `-32601`. ACP's baseline requires `session/cancel`; the line client does not send it, and the key that does arrives with #6.

Framing: one UTF-8 JSON message per line, ended by `\n`. A line over 16 MiB closes the connection and is logged.

### Transport

| Item | Linux and macOS | Windows |
| --- | --- | --- |
| Endpoint | `~/.kiri/daemon.sock`, mode `0600` | `\\.\pipe\kiri-<owner SID>` |
| Enclosing protection | `~/.kiri` at mode `0700`; a wider mode is reset at daemon start | DACL granting access to the owner SID only; `first_pipe_instance`; remote clients rejected |
| Check on each connection | The daemon reads the peer's uid and closes the connection when it differs from its own, without answering | The client reads the pipe's owner and refuses to send anything when it is another SID |
| Single instance | Exclusive lock on `~/.kiri/daemon.lock` | The same |
| Log | `~/.kiri/logs/daemon.jsonl`, mode `0600` | The same, inside the user's profile |

### Request

`initialize`

| Field | Type | Required | Validation |
| --- | --- | --- | --- |
| `protocolVersion` | integer | Yes | The daemon always answers `1`, as ACP specifies |
| `clientCapabilities` | object | No | Ignored |
| `clientInfo` | object | No | Ignored |

`session/new`

| Field | Type | Required | Validation |
| --- | --- | --- | --- |
| `cwd` | string | Yes | Absolute path. Comes from the client's working directory |
| `mcpServers` | array | Yes | Accepted and ignored until the extensions milestone |
| `additionalDirectories` | array | No | Accepted and ignored |

`session/prompt`

| Field | Type | Required | Validation |
| --- | --- | --- | --- |
| `sessionId` | string | Yes | A session created on this same connection |
| `prompt` | array of content blocks | Yes | `text` and `resource_link` only; a link becomes its URI in the text. At least one non-blank character. Comes from the user's line |

```json
{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"019a1f6e-...","prompt":[{"type":"text","text":"Explain this error"}]}}
```

### Response

`initialize` answers `protocolVersion: 1`, default `agentCapabilities`, an empty `authMethods`, and `agentInfo` with name `kiri` and the binary's version.

`session/new` answers `sessionId`, a UUIDv7 string.

`session/prompt` sends `session/update` notifications, then answers `stopReason`:

```json
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"019a1f6e-...","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"The error"}}}}
{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}
```

| Stream outcome | `stopReason` |
| --- | --- |
| `finish_reason` `stop` or `tool_calls`, or `[DONE]` with no `finish_reason` | `end_turn` |
| `finish_reason` `length` | `max_tokens` |
| `finish_reason` `content_filter` | `refusal` |
| `session/cancel` received | `cancelled` |

A stream that fails answers a JSON-RPC error from the [Errors](#errors) table.

### Environment

`crates/kiri-core/src/env.rs` is the only file that reads the environment. The daemon reads it once, at start, and inherits it from the client that spawned it.

| Variable | Type | Required | Validation |
| --- | --- | --- | --- |
| `KIRI_BASE_URL` | URL | Yes | `http` or `https`; up to 2048 bytes |
| `KIRI_API_KEY` | string | Yes | Not empty; up to 4096 bytes |
| `KIRI_MODEL` | string | Yes | Not empty; up to 256 bytes |
| `KIRI_REASONING_EFFORT` | string | No | One of the model's values in the catalog; case-sensitive |
| `KIRI_THINKING` | string | No | `on` or `off`; the model must have a switch in the catalog |
| `KIRI_REQUEST_PARAMS` | JSON object | No | Up to 64 KiB; cannot set `model`, `messages`, or `stream` |

### Native catalog

`crates/kiri-core/src/provider/catalog.rs`, a table in the binary. A model matches by its exact `KIRI_MODEL` value, so the same model behind a router, under another ID, takes the generic path. The catalog adds to the request only what the user set, except for the reasoning replay.

| Models | `KIRI_REASONING_EFFORT` | Sent as | `KIRI_THINKING` | Sent as | Reasoning replayed in history |
| --- | --- | --- | --- | --- | --- |
| `deepseek-v4-pro`, `deepseek-flash` | `low`, `high`, `max` | `reasoning_effort` | `on`, `off` | `thinking: {"type": "enabled"\|"disabled"}` | Yes |
| `kimi-k3.1-pro` | `low`, `high`, `max` | `reasoning_effort` | none | — | Yes |
| `glm-5.3`, `glm-5.3-flash` | `low`, `high`, `max` | `reasoning_effort` | none | — | Yes |
| `qwen3.8-max`, `qwen3.8-flash` | none | — | `on`, `off` | `enable_thinking: true\|false` | No |
| `gemini-4-argon`, `gemini-3.8-flash` | `low`, `medium`, `high` | `reasoning_effort` | none | — | No |

Claude, GPT, Grok, Muse, and MiniMax enter this table with #5, which brings the APIs they need.

### Request to the endpoint

`POST <KIRI_BASE_URL>/chat/completions`, with one trailing slash of the base URL removed.

| Part | Value | Source |
| --- | --- | --- |
| `Authorization` | `Bearer <key>` | `KIRI_API_KEY` |
| `Accept` | `text/event-stream` | Fixed |
| `User-Agent` | `kiri/<version>` | The binary's version |
| Body `model` | The model ID | `KIRI_MODEL` |
| Body `messages` | Earlier turns, then the new prompt, as `user` and `assistant`; no system message | The session's conversation |
| Body `stream` | `true` | Fixed |
| Other body keys | The catalog's settings, then the top-level keys of `KIRI_REQUEST_PARAMS`, which win on a clash | Environment |

No `tools` key is sent. An assistant message carries `reasoning_content` only when the catalog says so. A redirect is not followed. Timeouts: 10 seconds to connect, 60 seconds with no bytes while reading.

### Stream to ACP events

Only `choices[0]` is read, and a chunk with no choices is skipped.

| Chunk field | ACP update |
| --- | --- |
| `delta.reasoning_content`, or `delta.reasoning` when the first is absent | `agent_thought_chunk` |
| `delta.content` | `agent_message_chunk` |
| `delta.tool_calls[n]`, first fragment of index `n` | `tool_call`, title the function's name, kind `other`, status `pending`, ID the endpoint's or `call-<n>` |
| `delta.tool_calls[n].function.arguments`, once the stream ends | `tool_call_update`, status `failed`, `rawInput` the joined arguments |

### Line client

`kiri` with no arguments, in `crates/kiri-tui`. #4 replaces it.

| Element | Text |
| --- | --- |
| Input prompt | `>` and a space |
| Start of a reasoning run | `[reasoning]` and a space |
| Tool call appears | `[tool call] <name>` |
| Tool call's arguments arrive | `[tool call] <name> <arguments>` |
| Turn ended with a tool call | "Kiri has no tools yet, so this call did not run." |
| `max_tokens` | "The model reached its output limit." |
| `refusal` | "The model refused to answer." |
| Failure | `error: <message>` on stderr |

Text prints as it arrives. A change between reasoning and text starts a new line. A blank line is ignored. Ctrl-D ends the client with exit code 0, and Ctrl-C ends it the default way. A failure before the first prompt, or a lost connection to the daemon, ends it with exit code 1; a failed turn returns to the prompt.

### Roles and privileges

| Role | Permission | Note |
| --- | --- | --- |
| The OS user who owns the daemon | Every method | There is no other role. The transport enforces it |

---

## Business Rules

### 1. Authorization

Only the daemon's OS user reaches it. Another user's connection fails at the file permissions, or at the peer check when those were widened, and gets no answer. On Windows the DACL refuses it. A client that finds a pipe owned by another SID, or that the OS denies access to a pipe under its own SID's name, stops at once with "The Kiri daemon pipe belongs to another user. Refusing to connect." A refused connection does not count as a client, so it neither keeps the daemon alive nor ends it. A daemon that cannot create `~/.kiri` or reset its mode, or set the socket's, does not start.

### 2. Daemon on demand

The client tries to connect. When that fails it spawns its own executable as `kiri daemon`, detached from the terminal, with stdin, stdout, and stderr closed, and retries every 50 ms for up to 5 seconds. When nothing has answered 500 ms after a spawn, it spawns again. The same 5 seconds cover a connection that closes before `initialize` answers. On timeout it stops with "Could not connect to the Kiri daemon within 5 seconds. See `<log path>`." A failed spawn stops it with "Could not start the Kiri daemon: `<cause>`"

### 3. One daemon per user

A daemon takes the exclusive lock before anything else. The one that loses the lock prints "A Kiri daemon is already running." to stderr and exits with code 0. The one that holds it removes any leftover socket and binds. The OS releases the lock when a process dies, so a crash leaves nothing to clean by hand.

### 4. Daemon lifetime

`shortcut: the daemon exits when its last client leaves and sessions die with their connection, #3 replaces this with reattach and an idle period.`

The daemon exits with code 0 when its last connection closes, or 5 seconds after starting when no client has connected. Closing a connection cancels its running turn, closes the HTTP request, and discards its sessions. A user who fixes the environment and reopens Kiri always gets a daemon with the new values. Two terminals open at once share the values of the terminal that started the daemon.

### 5. Configuration at session start

The daemon validates the environment at start and keeps the result. An invalid result does not stop it: each `session/new` answers the error, and the client prints it and exits with code 1. The checks run in the order of the [Errors](#errors) table, and the first failure is the one reported. No message or log carries the value of a variable, except the invalid value of `KIRI_REASONING_EFFORT`.

### 6. Sessions and conversation

Each connection can open many sessions, and each session runs one turn at a time. A prompt during a running turn answers `-32602` with "A turn is already running in this session." A prompt for a session this connection did not create answers `-32002` with "Unknown session: `<id>`." The conversation is held in memory. A turn that ends in `end_turn` or `max_tokens` adds its prompt, its reply, and its reasoning to the conversation. Any other ending adds nothing, and neither does a reply with no text, so the next request stays valid: some endpoints may refuse an empty assistant message or two user messages in a row.

### 7. Streaming order and batching

`agent` keeps one open run of text or of reasoning, and sends it as one ACP event 10 ms after the run opens, when the kind changes, and when the turn ends, however it ends. A tool call flushes the open run and goes out at once. A cancelled or failed turn also sends the `tool_call_update` of each open tool call before its answer. The concatenated output of the events equals the provider's in the same order.

### 8. Tool calls do not run

A tool call the endpoint sends is shown and never executed. Its `tool_call_update` carries status `failed`, and the conversation keeps only the text and reasoning of that reply, or nothing when the reply has no text. #6 removes this rule.

### 9. Native settings and pass-through

A setting for a catalog model is checked at start and sent in that model's format. A catalog setting on a model outside the catalog is an error. `KIRI_REQUEST_PARAMS` works on every model, Kiri does not validate its contents, and an endpoint's refusal reaches the user through rule 10.

### 10. Failures carry their cause

Every failure reaches the user with the message of the [Errors](#errors) table. The API key is replaced by `***` in any text taken from an endpoint or a transport error, and that text is cut at 500 characters. No request is retried. The daemon and the client survive every failure of the network or of a file during a session.

### 11. Persistence and audit

- **Tables/columns changed:** none. Sessions and messages live in memory.
- **Audit:** none. The log records daemon start, daemon exit, connections refused, and failures with their HTTP status. It never records a prompt, a reply, reasoning, the API key, or `KIRI_REQUEST_PARAMS`. At daemon start, a log of 10 MiB or more moves to `daemon.jsonl.1`, replacing the previous one.
- **Events/integrations fired:** the `session/update` notifications of the contract, and the HTTP request to the endpoint.

---

## Errors

| Code | When | Message |
| --- | --- | --- |
| `-32603` | At `session/new`, a required variable is missing | "Set KIRI_BASE_URL, KIRI_API_KEY, and KIRI_MODEL before opening Kiri. Missing: `<names>`." |
| `-32603` | At `session/new`, a variable is over its limit | "`<name>` is longer than `<limit>` bytes." |
| `-32603` | At `session/new`, the base URL is invalid | "KIRI_BASE_URL must be an http or https URL." |
| `-32603` | At `session/new`, `KIRI_REQUEST_PARAMS` is not a JSON object | "KIRI_REQUEST_PARAMS must be a JSON object." |
| `-32603` | At `session/new`, `KIRI_REQUEST_PARAMS` sets a reserved key | "KIRI_REQUEST_PARAMS cannot set `<key>`, because Kiri sets it." |
| `-32603` | At `session/new`, `KIRI_THINKING` is neither `on` nor `off` | "KIRI_THINKING must be on or off." |
| `-32603` | At `session/new`, a catalog setting on a model outside the catalog | "`<model>` is not in Kiri's catalog, so KIRI_REASONING_EFFORT and KIRI_THINKING do not apply. Use KIRI_REQUEST_PARAMS." |
| `-32603` | At `session/new`, an effort value the model does not accept | "`<model>` accepts these KIRI_REASONING_EFFORT values: `<values>`. Got: `<value>`." |
| `-32603` | At `session/new`, an effort on a catalog model with none | "`<model>` has no reasoning effort setting in Kiri. Unset KIRI_REASONING_EFFORT, or use KIRI_REQUEST_PARAMS." |
| `-32603` | At `session/new`, a thinking switch on a catalog model with none | "`<model>` has no thinking switch in Kiri. Unset KIRI_THINKING, or use KIRI_REQUEST_PARAMS." |
| `-32602` | `cwd` is not absolute | "cwd must be an absolute path." |
| `-32602` | The prompt has no non-blank text | "The prompt is empty." |
| `-32602` | The prompt has another block type | "Kiri accepts only text and resource links in a prompt." |
| `-32602` | A turn is running in the session | "A turn is already running in this session." |
| `-32002` | The session is not on this connection | "Unknown session: `<id>`." |
| `-32603` | HTTP 401 or 403 | "The endpoint rejected the API key (HTTP `<status>`)." |
| `-32603` | HTTP 429 | "The endpoint is limiting requests (HTTP 429): `<endpoint text>`" |
| `-32603` | Another 3xx or 4xx | "The endpoint rejected the request (HTTP `<status>`): `<endpoint text>`" |
| `-32603` | HTTP 5xx | "The endpoint failed (HTTP `<status>`): `<endpoint text>`" |
| `-32603` | Connection or TLS failure | "Could not reach `<base URL>`: `<cause>`" |
| `-32603` | No bytes for 60 seconds, or no connection in 10 | "The endpoint sent nothing for `<seconds>` seconds." |
| `-32603` | The stream ends with no `finish_reason` and no `[DONE]` | "The endpoint closed the stream before the turn finished." |
| `-32603` | A chunk carries an `error` object | "The endpoint failed during the stream: `<endpoint text>`" |
| `-32603` | A chunk is not valid JSON | "The endpoint sent a chunk Kiri cannot read: `<cause>`" |

Failures with no JSON-RPC code, printed by the client:

| When | Message |
| --- | --- |
| The socket path is over the system's limit, 104 bytes on macOS and 108 on Linux | "The path `<path>` is too long for a Unix socket. The limit on this system is `<limit>` bytes." |
| The home directory cannot be found | "Kiri cannot find your home directory." |
| The spawn of the daemon fails | "Could not start the Kiri daemon: `<cause>`" |
| `kiri daemon` cannot create `~/.kiri` or reset its mode. Printed by the daemon and logged; a client that spawned it reports the 5-second timeout | "Could not secure `<path>`: `<cause>`" |
| No answer within 5 seconds at start | "Could not connect to the Kiri daemon within 5 seconds. See `<log path>`." |
| The pipe belongs to another SID, or the OS denies access to it | "The Kiri daemon pipe belongs to another user. Refusing to connect." |
| The connection closes, or stays silent for 70 seconds, during a turn | "The connection to the Kiri daemon was lost." |

## Side Effects

- **Persistence:** creates `~/.kiri` at mode `0700`, `~/.kiri/logs/daemon.jsonl` and `~/.kiri/daemon.lock` at mode `0600`, and on Linux and macOS `~/.kiri/daemon.sock` at mode `0600`. Nothing about a session is written.
- **Concurrency:** turns of different sessions run at the same time. Two daemons starting together resolve by the lock. A client connecting while a daemon exits sees a closed connection and follows rule 2.
- **Transaction:** a turn enters the conversation whole, at its end, or not at all.

---

## Acceptance Scenarios (Gherkin)

All scenarios pass on Windows, Linux, and macOS (NFR-FLEX-01.1).

### Scenario 1 — First prompt with no daemon running (happy path)

```gherkin
Given no daemon is running
And KIRI_BASE_URL, KIRI_API_KEY, and KIRI_MODEL are valid
When the user runs kiri and sends a prompt
Then a daemon starts with no other command
And the reply's text appears in pieces before the turn ends
And the client shows the input prompt again
```

### Scenario 2 — A second client shares the daemon (alternative)

```gherkin
Given a client is connected to a running daemon
When the user runs kiri in another terminal
Then no second daemon starts
And the new client gets its own session
And a turn in each session streams at the same time
```

### Scenario 3 — Two clients start at the same moment (alternative)

```gherkin
Given no daemon is running
When two clients start together
Then exactly one daemon holds the lock
And both clients reach it
```

### Scenario 4 — Leftover socket after a crash (alternative)

```gherkin
Given a daemon was killed and left its socket file behind
When the user runs kiri
Then a new daemon starts and the client connects
```

### Scenario 5 — The last client leaves (alternative)

```gherkin
Given one client is connected with a turn running
When the client closes
Then the HTTP request to the endpoint closes
And the daemon exits with code 0
```

### Scenario 6 — No client ever connects (exception)

```gherkin
Given a daemon started
When 5 seconds pass with no connection
Then the daemon exits with code 0
```

### Scenario 7 — The daemon cannot be reached (exception)

```gherkin
Given the daemon cannot start
When the user runs kiri
Then within 5 seconds the client prints "Could not connect to the Kiri daemon within 5 seconds. See <log path>."
And exits with code 1
Given the OS refuses to spawn the executable
When the user runs kiri
Then the client prints "Could not start the Kiri daemon: <cause>"
And exits with code 1
```

### Scenario 8 — Another OS user is refused (denied authorization)

```gherkin
Given the daemon of one OS user is running
When a second real OS user tries to connect
Then 0 connections succeed
```

### Scenario 9 — The peer check holds when permissions are widened (denied authorization)

```gherkin
Given on Linux or macOS ~/.kiri and the socket were made world-accessible
When a second real OS user connects and sends initialize
Then the daemon closes the connection with no answer
And logs the refused connection
```

### Scenario 10 — A pipe owned by another user is refused (denied authorization)

```gherkin
Given on Windows another SID owns the pipe name
When the user runs kiri
Then the client sends nothing
And prints "The Kiri daemon pipe belongs to another user. Refusing to connect."
```

### Scenario 11 — A later prompt carries the conversation (alternative)

```gherkin
Given a session whose first turn ended in end_turn
When the user sends a second prompt
Then the request's messages hold the first prompt, the first reply, and the second prompt
```

### Scenario 12 — Reasoning, text, and tool calls in arrival order (happy path)

```gherkin
Given an endpoint streams reasoning, then text, then a tool call
And it holds each part back until the client has shown the part before it
When the user sends a prompt
Then the client shows them in that order before the turn ends
```

### Scenario 13 — Both reasoning fields are read (alternative)

```gherkin
Given one endpoint sends delta.reasoning_content and another sends delta.reasoning
When the user sends a prompt to each
Then both arrive as agent_thought_chunk
```

### Scenario 14 — A tool call does not run (exception)

```gherkin
Given an endpoint sends a tool call in fragments
When the stream ends
Then the client shows the name and the joined arguments
And prints "Kiri has no tools yet, so this call did not run."
And the next request's history holds no tool call
```

### Scenario 15 — Batching keeps the content (alternative)

```gherkin
Given an endpoint sends 1000 text deltas at 1 ms intervals
When the turn ends
Then fewer than 500 agent_message_chunk events were sent
And their concatenated text equals the endpoint's
```

### Scenario 16 — Native effort (happy path)

```gherkin
Given KIRI_MODEL is deepseek-v4-pro and KIRI_REASONING_EFFORT is high
When the user sends a prompt
Then the request body carries reasoning_effort "high"
```

### Scenario 17 — Native thinking switch (alternative)

```gherkin
Given KIRI_MODEL is qwen3.8-max and KIRI_THINKING is off
When the user sends a prompt
Then the request body carries enable_thinking false
```

### Scenario 18 — Reasoning replayed by catalog (alternative)

```gherkin
Given a finished turn with reasoning
When the next prompt goes to kimi-k3.1-pro
Then the assistant message carries reasoning_content
And when the model is qwen3.8-max or outside the catalog, it does not
```

### Scenario 19 — An effort the model does not accept (exception)

```gherkin
Given KIRI_MODEL is glm-5.3 and KIRI_REASONING_EFFORT is medium
When the user runs kiri
Then the client prints "glm-5.3 accepts these KIRI_REASONING_EFFORT values: low, high, max. Got: medium."
And exits with code 1, with no request sent
```

### Scenario 20 — A setting with no equivalent on a catalog model (exception)

```gherkin
Given KIRI_MODEL is qwen3.8-max and KIRI_REASONING_EFFORT is low
When the user runs kiri
Then the client prints "qwen3.8-max has no reasoning effort setting in Kiri. Unset KIRI_REASONING_EFFORT, or use KIRI_REQUEST_PARAMS."
Given KIRI_MODEL is glm-5.3 and KIRI_THINKING is on
When the user runs kiri
Then the client prints "glm-5.3 has no thinking switch in Kiri. Unset KIRI_THINKING, or use KIRI_REQUEST_PARAMS."
```

### Scenario 21 — A catalog setting on another model (exception)

```gherkin
Given KIRI_MODEL is outside the catalog and KIRI_THINKING is on
When the user runs kiri
Then the client prints "<model> is not in Kiri's catalog, so KIRI_REASONING_EFFORT and KIRI_THINKING do not apply. Use KIRI_REQUEST_PARAMS."
```

### Scenario 22 — Pass-through on any model (happy path)

```gherkin
Given KIRI_MODEL is outside the catalog
And KIRI_REQUEST_PARAMS is {"chat_template_kwargs":{"enable_thinking":true},"temperature":0.2}
When the user sends a prompt
Then the request body carries both keys unchanged at its top level
```

### Scenario 23 — Pass-through overrides the catalog (alternative)

```gherkin
Given KIRI_MODEL is deepseek-v4-pro and KIRI_REASONING_EFFORT is high
And KIRI_REQUEST_PARAMS is {"reasoning_effort":"max"}
When the user sends a prompt
Then the request body carries reasoning_effort "max"
```

### Scenario 24 — Invalid pass-through (exception)

```gherkin
Given KIRI_REQUEST_PARAMS is [1,2]
When the user runs kiri
Then the client prints "KIRI_REQUEST_PARAMS must be a JSON object."
Given KIRI_REQUEST_PARAMS is {"stream":false}
When the user runs kiri
Then the client prints "KIRI_REQUEST_PARAMS cannot set stream, because Kiri sets it."
```

### Scenario 25 — A parameter the endpoint refuses (exception)

```gherkin
Given the endpoint answers HTTP 400 with the text "unknown parameter" to a pass-through key
When the user sends a prompt
Then the client prints "error: The endpoint rejected the request (HTTP 400): unknown parameter"
And returns to the input prompt
And the next request's history does not hold the failed prompt
```

### Scenario 26 — Missing or invalid configuration (exception)

```gherkin
Given KIRI_API_KEY is unset
When the user runs kiri
Then the client prints "Set KIRI_BASE_URL, KIRI_API_KEY, and KIRI_MODEL before opening Kiri. Missing: KIRI_API_KEY."
And exits with code 1
Given KIRI_BASE_URL is ftp://example.com
When the user runs kiri
Then the client prints "KIRI_BASE_URL must be an http or https URL."
Given KIRI_MODEL is 257 bytes long
When the user runs kiri
Then the client prints "KIRI_MODEL is longer than 256 bytes."
Given KIRI_THINKING is auto
When the user runs kiri
Then the client prints "KIRI_THINKING must be on or off."
```

### Scenario 27 — A fix takes effect at the next open (alternative)

```gherkin
Given a client exited because KIRI_API_KEY was unset
When the user sets it and runs kiri at once
Then the new daemon reads the new value
And the prompt is accepted
```

### Scenario 28 — Protocol misuse (exception)

```gherkin
Given a connected client
When it sends session/new with a relative cwd
Then it gets -32602 and "cwd must be an absolute path."
When it sends a blank prompt
Then it gets -32602 and "The prompt is empty."
When it sends an image block
Then it gets -32602 and "Kiri accepts only text and resource links in a prompt."
When it sends a prompt during a running turn
Then it gets -32602 and "A turn is already running in this session."
When it sends a prompt for a session of another connection
Then it gets -32002 and "Unknown session: <id>."
When it sends an unknown method
Then it gets -32601
When it sends a line over 16 MiB
Then the connection closes and the daemon stays up
```

### Scenario 29 — A resource link becomes text (alternative)

```gherkin
Given a prompt with a text block and a resource_link block
When the client sends it
Then the user message in the request holds the text and the link's URI
```

### Scenario 30 — Cancel (alternative)

```gherkin
Given a running turn
When the client sends session/cancel
Then the HTTP request closes
And session/prompt answers stopReason cancelled
And the conversation is unchanged
```

### Scenario 31 — Endpoint refuses by status (exception)

```gherkin
Given the endpoint answers <status>
When the user sends a prompt
Then the client prints <message>
And returns to the input prompt

Examples:
| status | message |
| 401 | error: The endpoint rejected the API key (HTTP 401). |
| 403 | error: The endpoint rejected the API key (HTTP 403). |
| 429 | error: The endpoint is limiting requests (HTTP 429): slow down |
| 302 | error: The endpoint rejected the request (HTTP 302): |
| 503 | error: The endpoint failed (HTTP 503): overloaded |
```

### Scenario 32 — Network failure (exception)

```gherkin
Given nothing listens at KIRI_BASE_URL
When the user sends a prompt
Then the client prints "error: Could not reach <base URL>: <cause>"
And the daemon stays up
```

### Scenario 33 — Idle endpoint (exception)

```gherkin
Given an endpoint accepts the connection and sends nothing
When 60 seconds pass
Then the client prints "error: The endpoint sent nothing for 60 seconds."
Given an endpoint that never accepts the connection
When 10 seconds pass
Then the client prints "error: The endpoint sent nothing for 10 seconds."
```

### Scenario 34 — Broken streams (exception)

```gherkin
Given an endpoint closes the stream with no finish_reason and no [DONE]
When the user sends a prompt
Then the client prints "error: The endpoint closed the stream before the turn finished."
Given a chunk carries an error object with the text "overloaded"
When the user sends a prompt
Then the client prints "error: The endpoint failed during the stream: overloaded"
Given a chunk is not valid JSON
When the user sends a prompt
Then the client prints "error: The endpoint sent a chunk Kiri cannot read: <cause>"
```

### Scenario 35 — Output limit and refusal (alternative)

```gherkin
Given the stream ends with finish_reason length
When the turn ends
Then the client prints "The model reached its output limit."
And the partial reply enters the conversation
Given the stream ends with finish_reason content_filter
When the turn ends
Then the client prints "The model refused to answer."
And the conversation is unchanged
```

### Scenario 36 — The daemon dies during a turn (exception)

```gherkin
Given a running turn
When the daemon is killed, or sends nothing for 70 seconds
Then the client prints "error: The connection to the Kiri daemon was lost."
And exits with code 1
```

### Scenario 37 — The key never leaks (exception)

```gherkin
Given an endpoint echoes the API key in its error text
When the user sends a prompt
Then the client prints *** in its place
And the log file holds no API key, prompt, reply, reasoning, or KIRI_REQUEST_PARAMS
```

### Scenario 38 — Paths outside what the daemon can use (exception)

```gherkin
Given the home path makes the socket path longer than the system's limit
When the user runs kiri
Then the client prints "The path <path> is too long for a Unix socket. The limit on this system is <limit> bytes."
And exits with code 1
Given the OS reports no home directory
When the user runs kiri
Then the client prints "Kiri cannot find your home directory."
And exits with code 1
```

### Scenario 39 — A second daemon by hand (alternative)

```gherkin
Given a daemon is running
When the user runs kiri daemon
Then it prints "A Kiri daemon is already running."
And exits with code 0
```

---

## Architecture impact

- **`kiri-core`:** the `protocol` module (already in the components diagram) holds the JSON-RPC layer and the transport. `env.rs` is new. `provider/` holds `openai_compatible.rs` and `catalog.rs`, with no `Provider` trait until #5 brings a second implementation. The public surface grows by two entry points the other crates need: `daemon::run`, called by `kiri daemon`, and `protocol::connect`, called by `kiri`.
- **`kiri`:** `kiri` and `kiri daemon` exist. `kiri` spawns the daemon. Until #3, `kiri daemon` exits when its last client leaves.
- **`kiri-tui`:** holds the line client until #4 replaces it. It depends on the protocol types and on `protocol::connect`'s connection.
- **`xtask`:** a new workspace member with the cross-user check, run by CI. It ships in no release.
- **Deployment:** the first CI workflow, on Windows, Linux, and macOS. `~/.kiri` gains `daemon.sock`, `daemon.lock`, and `logs/`.
- **Runtime:** a new scenario, "Starting the daemon". "A prompt turn" stays as written for M1, and this epic builds steps 1 to 5 and the turn-end event of step 9, with no persistence.
- **Concepts:** Streaming gains the reasoning replay and the catalog. Security's owner-only transport row gains the mechanisms of the [Transport](#transport) table.
- **ADR 0002:** adds `futures-util` 0.3.34, the `Stream` consumer that `reqwest`'s `stream` and `eventsource-stream` need.

## Data model impact

- **Session:** gains its identity and attributes: `id` (UUIDv7), `cwd`, and `messages`. Held in memory; no file.
- **Message:** gains `role` (`user` or `assistant`), `text`, and optional `reasoning`. Held in memory; no file.
- **Provider:** one instance per daemon, with `base_url`, `api_key`, `model`, and `request_params`, from the environment. Held in memory; no file.
- Storage on disk gains no entity. The physical files of this epic are the lock, the socket, and the log.

## Out of Scope

- Reattach, a session outliving its terminal, the idle period, and client and daemon versions: #3.
- The inline interface, the seal, the palette, and live input during a turn: #4.
- The Anthropic and Responses APIs, and with them the native entries for Claude, GPT, Grok, Muse, and MiniMax: #5.
- In-app setup, a configuration file, stored credentials, a per-provider idle timeout (NFR-REL-02.1 allows the setting; this epic fixes the default of 60 seconds), switching mid-session, and a local server with no key: #5.
- Sending tools, running them, a system prompt, and the interrupt key: #6.
- Writing sessions to disk: FR-CNT-03 is Won't for this release.
- Context limits in the catalog and compaction: #10.
- Images in the prompt: #11.
- `kiri -p` and `kiri acp`: Later.
- Automatic retries: no requirement asks for them, and #12 reviews failure handling across the system.
- A native setting for Qwen's `thinking_budget`, and Z.ai's and Qwen's reasoning preservation switches: they work through `KIRI_REQUEST_PARAMS`.
- Updating the catalog outside a Kiri release.

## Task Breakdown

| # | Issue | Title | Scope | Acceptance criterion | Depends on |
| --- | --- | --- | --- | --- | --- |
| 1 | #14 | Start the daemon on demand over a local socket | Cargo workspace with `kiri-core`, `kiri-tui`, `kiri`, `xtask`; CI on three systems; `gates` skill and `AGENTS.md` conventions; `protocol` (JSON-RPC, socket, pipe), lock, log, `initialize`, spawn, exit rules | Scenarios 3, 4, 6, 7, 38, 39 | — |
| 2 | #15 | Restrict the daemon to its OS user | File modes, peer check, DACL, pipe owner check; `xtask` cross-user check in CI on three systems | Scenarios 8, 9, 10 | 1 |
| 3 | #16 | Stream a text answer from an OpenAI-compatible endpoint | `env.rs`, `session`, `agent`, `provider/openai_compatible.rs`, line client in `kiri-tui`, `session/new`, `session/prompt`, `session/cancel`, conversation in memory | Scenarios 1, 2, 5, 11, 26, 27, 28, 29, 30 | 1 |
| 4 | #17 | Show reasoning and tool calls in arrival order | Delta parsing, tool call events, 10 ms batching, client's reasoning and tool call output | Scenarios 12, 13, 14, 15 | 3 |
| 5 | #18 | Add the native catalog and request parameters | `provider/catalog.rs`, the remaining variables of `env.rs`, request body, reasoning replay | Scenarios 16, 17, 18, 19, 20, 21, 22, 23, 24 | 4 |
| 6 | #19 | Report every failure with its cause | Error mapping, timeouts, stop reasons, key redaction, client's failure paths | Scenarios 25, 31, 32, 33, 34, 35, 36, 37 | 5 |
