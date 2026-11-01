# Streaming

Provider output flows through one local pipeline, the same for every provider:

1. Raw SSE bytes arrive from the provider.
2. **provider** turns them into provider events.
3. **agent** groups provider events into batches of about 10 ms.
4. Each batch leaves the engine as ACP events.

Each stage does one transformation. Batching keeps redraws off the per-token path, which is what
[NFR-PERF-01](../../requirements/non-functional/performance.md#nfr-perf-01) needs. With the Claude
subscription the CLI's stream enters at stage 2.

A batch opens with its first delta and leaves 10 ms later, even if the stream has gone quiet. Whatever ends the
turn, including a cancel or a failure, the open batch and the `failed` update of each open tool call leave before
the answer. A reply with no text adds nothing to the conversation.

Stage 1 starts from a request the catalog and the user shape
([FR-PRV-10](../../requirements/functional/providers.md#fr-prv-10)). A model in the catalog takes its reasoning
settings in its own format, and `KIRI_REQUEST_PARAMS` adds any key to the body on any model. The conversation
keeps each reply's reasoning, and the models the catalog marks receive it back in later requests.

## When a turn fails

Every failure reaches the user with the message from the spec's error table. Text taken from an endpoint or a
transport error has the API key replaced by `***` and is then cut at 500 characters. Replies, reasoning, and tool
call arguments are not masked. The log records the kind of failure and its HTTP status, because an endpoint's text
can echo the prompt.

A request has 10 seconds to connect and 60 seconds between bytes. The 60-second timer restarts when the headers
arrive, so a healthy turn can pass 60 seconds with nothing for the client. SSE comments and empty deltas also reset
it without producing an ACP event, so that silence has no upper limit.

The client gives up after 70 seconds of silence from the daemon, which is why the daemon sends `_kiri/alive` every
30 seconds during a turn. The notification carries nothing, and the client uses it only to restart its 70 seconds.
Those 70 seconds therefore detect a daemon that has hung, and they say nothing about the endpoint.
