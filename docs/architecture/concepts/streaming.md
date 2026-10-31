# Streaming

Provider output flows through one local pipeline, the same for every provider:

1. Raw SSE bytes arrive from the provider.
2. **provider** turns them into provider events.
3. **agent** groups provider events into batches of about 10 ms.
4. Each batch leaves the engine as ACP events.

Each stage does one transformation. Batching keeps redraws off the per-token path, which is what
[NFR-PERF-01](../../requirements/non-functional/performance.md#nfr-perf-01) needs. With the Claude
subscription the CLI's stream enters at stage 2.
