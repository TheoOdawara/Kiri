# ADR 0023 — recall_memory cross-store dedup: project wins, with an observable drop

- Status: Accepted
- Date: 2026-07-05
- Amends: ADR 0018 (`0018-memory-store-model.md`) — adds a rule to its existing "Store priority in
  recall" section, rather than restating the three-store model or the gitignore policy it already covers.

## Context

Audited as issue #11. `recall_memory` with `scope: "both"` rendered project and shared hits as two fully
independent lists. A fact learned into both scopes — the distiller classifying the same knowledge twice
across sessions, or a user running `remember` against both scopes — surfaced twice, wasting context
budget and reading as two facts corroborating each other rather than one.

Issue #11 also carried a criterion that project memory should be gitignored. That is already settled by
ADR 0018's Gitignore Policy table (`.kiri/memory/*.md` is tracked; only the derived `embeddings.json` is
not) — restated here only to close #11 against the model actually shipped, not reopened.

## Decision

`recall_memory` (`scope: "both"`) now drops a shared-store hit that near-duplicates a project-store hit
before rendering — project wins on provenance. "Near-duplicate" reuses the distiller's existing check
(normalized equality, or Jaccard token-overlap ≥ 0.8), moved from `distill.rs` into
`memory::domain::similarity` so both call sites share one definition instead of two copies drifting
apart. The `Memory` trait is unchanged: the dedup is a presentation concern of the `recall_memory` tool,
not a capability the trait needs to expose.

**The drop is never silent.** When entries are dropped, the tool's own output states the count (e.g. "(1
shared entry omitted as duplicate of project memory)") — mirroring the distiller's
`DistillReport.skipped`, so nothing disappears from the model's context without a trace it can act on.

**Accepted residual risk.** The Jaccard check is a bag-of-words measure blind to negation and word order:
a project-memory entry differing from a shared one by a single token (a negation flip, a changed number)
on a long-enough entry can still clear 0.8. Project-memory entries are normally written by the model
itself via the `remember` tool during a session; if an earlier turn were compromised (e.g. a
prompt-injection payload steering a `remember` call), a crafted project entry could suppress a specific,
different, legitimate shared entry from a combined recall. This requires an existing prompt-injection
foothold, and only omits that entry from this one tool's rendered output (a `scope: "shared"`-only
recall still shows it; the shared store itself is untouched) — but it is real and not fixed here.
Tracked as issue #55 (`security-debt`): whether cross-store dedup should require a stricter threshold
than same-scope write-time dedup, since the two cross a different trust boundary. This is independent of
project memory being versioned in git (ADR 0018's deliberate choice, for portable cross-session,
cross-contributor knowledge) — the risk is in what the model itself can write via `remember`, the same
as for any other tool call, not in the store's persistence format.

## Consequences

- `memory::domain::similarity::is_near_duplicate` is now the one definition of near-duplicate text, used
  by the `Distiller`'s write-time dedup and `recall_memory`'s read-time cross-store dedup.
- `recall_memory`'s `limit` argument is now clamped to `MAX_LIMIT` (50) after parsing. The dedup pass is
  O(project × shared) `is_near_duplicate` calls; an unbounded model-supplied limit against a since-grown
  store would have turned one tool call into real CPU cost inside the single-threaded engine. Closed as a
  side effect of this change rather than deferred, since the fix is a one-line clamp.
- No trait/port signature changed; `Memory`, `MemoryStore`, `SharedStore` are untouched.
- Closes #11.

## Update — 2026-07-05 — cross-store dedup tightened to exact-normalized equality (audit #55)

The accepted residual risk above is now closed rather than deferred. `recall_memory`'s cross-store dedup
switched from `is_near_duplicate` (Jaccard token-overlap ≥ 0.8) to a new
`memory::domain::similarity::is_exact_normalized_duplicate` (case/whitespace-normalized equality only, no
token-overlap slack). The two call sites now use different strictness because they cross different trust
boundaries: the `Distiller`'s write-time same-scope dedup compares entries the harness itself just derived
in the same batch — a fuzzy reword match there is a quality nicety, not a security surface — so it keeps
`is_near_duplicate`. `recall_memory`'s cross-store comparison pits a project entry (writable by the model
itself, via `remember`, in a session that could have an earlier prompt-injection foothold) against a
shared entry from a different trust level; a Jaccard threshold there was gameable by a single crafted
token change. Exact-normalized equality has no such slack, at the cost of no longer catching a genuine
reword as a duplicate across stores — accepted, since a duplicate *listing* is a context-budget wrinkle,
not a security gap, and this was already the harder-to-hit case in practice (most cross-store duplicates
are the distiller writing the identical fact to both scopes, which normalizes exactly equal either way).

Locked by `is_exact_normalized_duplicate`'s own unit tests and by
`cross_store_reword_is_no_longer_dropped_as_a_duplicate` in `recall_memory.rs`, which pins the specific
behavior change: a reword that Jaccard would have dropped now survives. Closes #55.
