# 0004. Memory architecture and backends

- Status: accepted
- Date: 2026-08-04

## Context

Kiri must keep memory separate from project documentation while still
supporting the memory workflow that already works across the local Claude and
Codex setup. Memory needs distinct global and project scopes, optional
cross-harness availability, human-readable storage, and safe movement between
storage locations.

## Decision

### Architecture

Kiri adopts the existing `ai-memory` architecture as its behavioral model:

- The durable store is a curated Markdown vault.
- `index.md` is the default entry point and has a bounded size.
- Reads are lazy: open the index first, then only relevant notes.
- Memory is a hint to verify, never authoritative truth.
- Writes require a gate: the fact is durable, not derivable from the
  repository, confirmed or explicitly requested by the user, and not a
  duplicate.
- Each durable note is a human-editable Markdown file with YAML frontmatter.
  The frontmatter carries `id`, `type`, `project`, `visibility`, `owner`,
  `origin`, `copied_from`, `date`, and `updated_at`.
- Harness scratch is disposable and is promoted into durable memory only when
  it passes the write gate.
- Promotion moves the item instead of copying it, unless a shared consumer
  requires a snapshot.
- The vault keeps an index, a change log, and periodic lint for contradictions,
  stale notes, and orphaned notes.

Kiri reuses this architecture, not the configuration or data ownership of
Claude or Codex. Kiri does not read or migrate `~/.claude` or `~/.codex`.

### Scopes and visibility

- Global memory contains reusable facts, preferences, decisions, and lessons.
- Project memory contains facts specific to the active repository.
- Project memory has two independent channels:
  - `shared`: repository-visible and suitable for version control.
  - `private`: user-local and not automatically shared with the repository.
- Memory is contextual only. It cannot override instructions, configuration,
  workflow definitions, permissions, sandbox boundaries, or the `PolicyEngine`.

### Global backend

Exactly one global backend is active at a time:

- `cross_harness`: `~/ai-memory`, compatible with Kiri and other harnesses.
- `kiri_only`: `~/.kiri/ai-memory`, available only to Kiri.

The default is `kiri_only`. If `~/ai-memory` exists, Kiri only presents it as
an opt-in choice; it never activates or imports it automatically.

The backend choice is Kiri configuration. Kiri's global configuration remains
owned by `~/.kiri`; choosing the cross-harness memory backend is an explicit
opt-in to the compatible vault, not a change to Kiri's configuration discovery.

### Read and write policy

Memory availability is configurable per resource:

- `disabled`: do not read or write that memory resource.
- `silent`: read without interruption, write automatically, and show a short
  conversational notice describing what was stored.
- `explicit`: read without interruption, but show the proposed memory and ask
  before writing it.

Equivalent updates may replace an existing memory. A real conflict is never
silently overwritten. Kiri presents the conflict and lets the user update,
keep both, or discard the new item. Deletion is always explicit.

### Backend changes

Changing the active global backend is a migration, not a pointer change. Kiri
uses stable memory IDs and lineage metadata to classify each item:

- Kiri-exclusive items move physically to the new backend.
- Items used by another harness are copied to the new backend and remain in the
  original backend for that harness.
- Unknown usage is treated as shared and copied for safety.

Copies are independent snapshots with no automatic synchronization. When
returning to a backend, an unchanged snapshot is discarded in favor of its
original source. A changed snapshot is reintroduced as Kiri-owned memory, with
user input required only when a conflict exists.

The migration verifies the destination before changing the active backend. A
failed or incomplete migration leaves the original backend authoritative.

## Consequences

- Kiri can provide the proven `ai-memory` workflow without becoming dependent
  on Claude or Codex configuration semantics.
- Users can choose cross-harness memory without forcing every Kiri setting to
  be cross-harness.
- Versioned project memory and private project memory can coexist without
  confusing memory with documentation.
- Backend changes are explicit and recoverable, but require lineage metadata
  and a migration step.
- Snapshot copies can diverge; reconciliation is deliberately explicit rather
  than another background synchronization system.

## Alternatives considered

- A proprietary database was rejected because Markdown is inspectable,
  editable, versionable, and already proven by the existing vault.
- Loading the entire vault was rejected because it wastes context and makes
  stale or irrelevant memory authoritative by accident.
- Two active global backends were rejected because they create duplicate,
  contradictory sources of truth.
- Always moving shared files was rejected because it would break other
  harnesses using the cross-harness vault.
- Always copying files was rejected because it leaves unnecessary duplicate
  ownership and makes exclusive memory harder to manage.
