# ADR 0010 — Memory contexts and documentation as knowledge sources

- Status: Accepted
- Date: 2026-06-25

## Context

The harness had no durable knowledge: every session started blank, so decisions, patterns, and facts
learned in one turn were lost in the next, and nothing carried across sessions or projects. We want the
agent to (a) accumulate durable knowledge, (b) recall it on demand, and (c) fall back to the project's
own documentation when memory does not cover a question.

Three tiers of knowledge, in descending availability:

1. **Shared memory** — cross-project, always available regardless of the workspace.
2. **Project memory** — specific to the current repository.
3. **Project documentation** — richer, project-specific knowledge already written under `docs/`.

## Decision

Add a `memory` bounded context (`src/modules/memory/{domain,application,infrastructure}`) following the
modular-hexagonal architecture of ADR 0003.

**Domain.** `MemoryEntry` (UUID v7 id, `MemoryKind`, content, tags, optional `project_id`, timestamps)
and `MemoryKind` (decision, pattern, anti-pattern, snippet, heuristic, fact). `project_id_from_path`
derives a stable project id from the workspace path (blake3, 16 hex chars).

**Ports.** Domain ports `ProjectMemory` / `SharedMemory` model the persistence contract, trimmed to the
wired surface (`init`/`save`/`load`/`search`/`list`, plus `list_by_*`/`count` kept for the future UI and
the store/sync tests). Application ports collapse to a single base `MemoryStore`
(save/search/`list_by_*`/embedding-persistence/availability) with `SharedStore: MemoryStore` adding the
cross-project `list_by_project`; `MemoryPort` (with `MemoryPortImpl<P, S>`) unifies project + shared for
recall/remember.

**Adapters.**
- `FileProjectMemory` — Markdown files with YAML front-matter plus a JSON index, under
  `<workspace>/.kiri/memory/`. Human-readable and diffable, so project memory lives in the repo.
- `SqliteSharedMemory` — a single SQLite database at `~/.kiri/memory/shared.db`. SQLite (over a
  file-per-entry scheme) was chosen for the shared store because it scales to many cross-project
  entries with indexed queries, and a future memory-management GUI will read/edit it directly. The
  blocking `rusqlite` connection lives behind `Arc<Mutex<Connection>>` and every call runs on a
  blocking thread (`spawn_blocking`) so it never stalls the single-threaded TUI runtime.
- `DocsLibrary` — read-only search over the project's docs tree (default `<workspace>/docs`), returning
  ranked excerpts. It is the third, fallback tier.

**Access path = tools + auto-injection.**
- Tools advertised to the model: `recall_memory` (read-only), `remember` (write), `consult_docs`
  (read-only). They are wired alongside the file tools in `app::wire`.
- At session start, `app::wire` injects a bounded `# Relevant memory` digest (recent project + shared
  entries, capped by count and bytes) into the system prompt, so the agent is grounded without a tool
  call. The system prompt therefore became an owned `String` composed at wire time, and `wire` became
  `async` to run the stores' `init` and the digest queries.

**Degradation.** Memory is auxiliary: if a store's `init` fails it is surfaced on stderr and left
inert (`is_available() == false`), the tools report it, and the harness still starts.

## Consequences

- **Memory I/O lives outside the tool sandbox.** `memory/infrastructure` performs filesystem/SQLite I/O
  directly against `.kiri/memory` and `~/.kiri/memory`. This does not violate the sandbox invariant
  (ADR 0002/0003): the sandbox is the chokepoint for *agent-directed* file paths; the memory store is
  the harness's own data directory, not a path the model supplies. The `remember` tool never resolves
  arbitrary paths — it hands a `MemoryEntry` to the port. `consult_docs` is read-only over the docs
  root.
- Ports return `AgentError` (the WIP's `anyhow::Result` was aligned to the architecture contract;
  `anyhow` stays at the binary edge).
- **Accepted domain exception: clock + RNG in `MemoryEntry::new`.** The constructor reads the wall clock
  (`now_rfc3339`) and the RNG (`Uuid::now_v7`) directly in the domain layer. This is ratified as a
  documented exception rather than injecting a `Clock`/`IdGen` port for a single constructor (YAGNI); the
  impurity is confined to entry creation and the rest of the domain stays pure and I/O-free.
- New dependencies: `rusqlite` (bundled), `serde_yaml`, `time`, `uuid` (v7), `blake3`; `tempfile` as a
  dev-dependency.
- `KIRI_MEMORY=off` disables the whole context; `KIRI_DOCS_PATH` overrides the docs root.
- A memory-management GUI is planned. The speculative full-CRUD port surface was trimmed (Wave 5) to what
  the runtime, the digest, and the sync export/import actually call: the `delete`/`count_by_project`
  domain methods and the duplicate `*Store` tier were removed (decision: wire-only; restore the CRUD
  methods from git history when the UI is built), `list_by_*`/`count` are retained behind targeted
  `#[allow(dead_code)]` for the tests and the future UI, and the two application ports were folded into a
  single `MemoryStore` base + `SharedStore` extension.
