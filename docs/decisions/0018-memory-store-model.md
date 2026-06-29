# ADR 0018 — Memory Store Model

**Status:** Accepted  
**Date:** 2026-06-29  
**Relates to:** ADR 0010, ADR 0013, ADR 0014, ADR 0015

## Context

The memory subsystem was built incrementally, leading to a double-port architecture:

- `ProjectMemory` / `SharedMemory` traits — full persistence contracts (`init`, `save`, `load`, `list`, `list_by_*`, `save_embedding`, `embedded_candidates`)
- `MemoryStore` / `SharedStore` traits — reduced use-case surface (the subset `LayeredMemory` actually calls)
- `FileProjectStore` / `SqliteSharedStore` — adapters that exclusively delegated every call to the concrete types above

The delegation adapters carried no logic; they existed solely to expose the concrete stores as the reduced-surface port. This added 200+ LOC of pass-through code and required maintaining two parallel trait tiers for the same concept.

## Decision

**Eliminate the delegation adapters.** `FileProjectMemory` and `SqliteSharedMemory` now implement the `MemoryStore` and `SharedStore` use-case ports directly, alongside their existing `ProjectMemory` / `SharedMemory` persistence ports. `FileProjectStore` and `SqliteSharedStore` are deleted.

Both concrete types track availability via an `Arc<AtomicBool>` set to `true` on successful `init()`. Inert mode (memory disabled, or fallback in-memory DB) leaves the flag `false` — `is_available()` returns false without requiring the caller to supply an explicit flag.

### Trait hierarchy (unchanged)

| Trait | Surface | Used by |
|---|---|---|
| `ProjectMemory` | Full CRUD + `init`/`load`/`list` | `app.rs` (digest, init), future memory GUI |
| `SharedMemory` | Full CRUD + `init`/`load`/`list`/`count` | sync module, `app.rs` (digest, init, sync factory) |
| `MemoryStore` | `save`/`search`/`embedded_candidates`/`save_embedding`/`list_by_kind`/`list_by_tag`/`is_available` | `LayeredMemory<P>` bound |
| `SharedStore` | `MemoryStore` + `list_by_project` | `LayeredMemory<S>` bound |

### Store priority in recall

`recall_memory` queries by scope:
- `scope: "both"` (default): project store first (surfaced as "Project memory"), shared store second ("Shared memory")
- `scope: "project"` or `scope: "shared"`: single store, same provenance prefix

Semantic recall (cosine) leads; keyword fills the remainder. Both are scoped to the active embedder's model id — foreign-model vectors are never ranked.

### Distiller routing

The Distiller routes extracted entries by the `scope` field the LLM assigns:
- `scope: "project"` → `remember_project()` → `FileProjectMemory`
- `scope: "shared"` (default for most kinds) → `remember_shared()` → `SqliteSharedMemory`

No kind-based routing override is wired. The LLM's scope classification is the signal.

### Gitignore policy

| Path | Tracked? | Reason |
|---|---|---|
| `.kiri/memory/*.md` | Yes | Human-readable Markdown, portable across machines |
| `.kiri/memory/index.json` | Yes | Needed for recovery (orphan detection) |
| `.kiri/memory/decisions/*.md` | Yes | Same as above |
| `.kiri/memory/embeddings.json` | **No** | Machine-local derived data, re-derivable from content |

### Sync scope

`kiri sync` exports shared memory (`SqliteSharedMemory`) only. Project memory (`.kiri/memory/`) is intentionally out of sync scope — it travels with the repository, not the profile.

## Consequences

- `FileProjectStore` and `SqliteSharedStore` are removed. Any code that constructed them must be updated to use the concrete types directly.
- `LayeredMemory::new(project, shared)` now takes `FileProjectMemory` and `SqliteSharedMemory` directly (or any other `MemoryStore`/`SharedStore` implementor, including `InMemoryStore` for tests).
- The `list_by_kind`, `list_by_tag`, and `list_by_project` methods remain on the use-case ports (`MemoryStore`/`SharedStore`) with `#[allow(dead_code)]` — reserved for the future memory-management GUI.
