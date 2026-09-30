# ADR 0013 — Session persistence + automatic learning loop

- Status: Accepted
- Date: 2026-06-26

## Context

Conversations were ephemeral: `Conversation` is an in-memory `Vec<Message>`, discarded on exit or `/new`.
Nothing carried a session's work forward, and the only way knowledge reached the durable memory (ADR 0010)
was an explicit `remember` tool call by the model. Two goals followed: persist conversations so work
survives across runs and machines, and make the harness *get smarter the more it is used* without the user
having to ask it to remember things.

## Decision

### Session persistence — a new `session` bounded context

`src/modules/session/{domain,application,infrastructure}`, following the modular-hexagonal layout of
ADR 0003.

- **Domain.** `Session` (id, project_id, title, timestamps, `Vec<Message>`) and `SessionSummary` (the
  listing view). `Session` reuses the agent domain's `Message` directly — a one-directional
  session→agent dependency. The domain `Message` stays serde-free (ADR 0003); a `StoredMessage` serde DTO
  in the infrastructure layer owns the JSON mapping.
- **Port.** `SessionStore` (create / append_messages / set_title / latest_for_project / list_for_project /
  load / delete / is_available).
- **Adapter.** `SqliteSessionStore` at `~/.kiri/sessions.db`, mirroring `SqliteSharedMemory`'s
  `Arc<Mutex<Connection>>` + `spawn_blocking` + 5s-timeout pattern. Sessions are keyed by `project_id`
  (blake3 of the workspace path) so a workspace lists only its own — transcripts stay out of the project
  repo (privacy) and in the per-user `~/.kiri`.

The TUI runtime flushes only the new message tail after each turn settles (post-rollback), lazily creating
the row so an empty session never touches the DB. The **system seed is never persisted** — it is
regenerated per run with the current memory digest. `/resume` reopens the latest session; `/sessions`
lists them in a picker; opening one rebuilds the conversation and a render-only transcript.

### Automatic learning loop — end-of-session distillation

A `Distiller` use-case lives in the **memory** application layer (not a new context — YAGNI). It depends on
the `MemoryPort` (to write) and is handed a `CompletionProvider` at call time (so it always uses the live
adapter after a `/provider`/`/effort` swap). On a session boundary it renders a bounded transcript, asks
the model for a strict JSON array of durable entries (decisions, patterns, anti-patterns, snippets,
heuristics, facts, and **preferences**), validates and de-duplicates each, and writes them to memory.

It runs at `/new`, on a session switch (`/resume` / `/sessions`), on `/cd`, and at quit — driven as a
`select!` arm in the runtime (the provider future is `!Send`, never spawned), with a spinner and **Ctrl+C
to skip**, bounded by an internal 30s timeout. A `should_distill` gate (≥1 user + ≥1 non-empty assistant
message) keeps it off empty/aborted sessions. A failure or timeout surfaces a Notice and never blocks the
boundary or loses the already-persisted session.

### Continuous preference capture

Rather than a new tool or a brittle regex, the system prompt authorizes the model to call
`remember(kind="preference", scope="shared")` the moment the user states a durable preference. The
end-of-session distiller is the backstop for preferences captured live were missed. A new
`MemoryKind::Preference` carries them.

## Consequences

- Conversations survive restarts and become the substrate the harness learns from; recall improves with
  use. Both stores degrade to inert (never fatal), the same contract as memory.
- Distillation costs one extra LLM call per session boundary, bounded and skippable.
- `~/.kiri/sessions.db` is per-machine until the portable-profile sync (ADR 0015) carries it.
- Tradeoff: persistence flushes once after a turn settles, so a crash mid-turn loses that in-flight turn
  (which would be re-asked anyway) in exchange for a DB that always mirrors the resumable in-memory state.
