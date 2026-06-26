# ADR 0014 — Semantic recall via embeddings

- Status: Accepted
- Date: 2026-06-26

## Context

Memory recall (ADR 0010) was keyword-only: a case-insensitive substring match over content, tags, and
kind. That misses paraphrases — a query "how do we handle errors" would not surface an entry phrased
"prefer typed Result over panics". As the harness accumulates knowledge (ADR 0013), recall quality is the
multiplier on how much that knowledge actually helps; semantic similarity is the lever.

## Decision

### Port + adapter

A new `EmbeddingProvider` port (`embed(texts) -> Vec<Vec<f32>>` + `model()`), separate from
`CompletionProvider` because the active chat provider may be one (Anthropic) that exposes no embeddings
while embeddings point at another. Unlike the streaming chat port it is plain `Send` (no `EventSink`), so
the `Send` memory port can await it. The adapter (`OpenAiEmbeddingProvider`) POSTs `{base_url}/embeddings`,
reusing the chat adapter's timed client and `error_from_status`. The factory's `build_embedding_provider`
refuses an Anthropic profile.

### Configuration

A global-only `[embeddings]` section: `provider` (an existing provider id whose base_url + credential to
reuse) and `model`. Trusted layer only — an untrusted workspace must not redirect where memory text is
sent. `None` keeps recall keyword-only.

### Storage

- Shared memory (SQLite): an additive `entry_embeddings(entry_id, model, dim, vector BLOB)` table; vectors
  are little-endian f32 bytes, behind the same `Arc<Mutex<Connection>>` + `spawn_blocking` plumbing.
- Project memory (files): a sidecar `embeddings.json` (a derived cache, kept out of the human-readable
  `index.json`).

Embeddings are written eagerly on `remember` (best-effort — a failed embed only disables semantic recall
for that one entry; the entry is saved regardless).

### Ranking + degradation

Cosine similarity lives in a pure `memory/domain/similarity.rs` (brute-force over a bounded candidate set
— no vector-DB dependency at this scale). `MemoryPortImpl` holds an optional embedder: recall embeds the
query (under a short separate timeout), fetches the embedded candidates, ranks by cosine, and returns the
top-k. **Any** failure — embedder unconfigured, endpoint down, timeout, or an empty ranked set — falls
back transparently to the existing keyword search. The boot digest stays recency-based (no embedding call
at startup).

## Consequences

- Recall finds paraphrased knowledge, not just literal substrings, sharply improving how often the right
  memory surfaces. With no `[embeddings]` configured, behavior is unchanged (keyword).
- One embedding call per `remember` and one per recall when configured; both bounded and degrade to
  keyword on failure, so a flaky embeddings endpoint never breaks recall.
- Embedding vectors are machine-local derived data and are excluded from the portable sync (ADR 0015) —
  re-derivable on each machine from the synced content.
- Incidental fix: project-memory filenames now use the full entry id (a truncated UUID v7 prefix is a
  millisecond timestamp, so two same-kind entries saved in the same millisecond collided and one
  overwrote the other — a latent data-loss bug surfaced by the embeddings tests).
