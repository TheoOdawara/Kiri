# Kiri Remediation Plan

This directory holds the **master remediation plan** derived from the read-only code audit of Kiri (`docs/audit/`). The audit existed for one reason: Kiri is a harness built for real top-tier software engineering, so its **own** code must hold the same bar. This plan turns the audit's 175 findings (collapsed to **135 unique remediation items**) into five sequenced, shippable waves whose end state is code that is clean, fully human-readable, senior-level, production-ready, and fit to open-source — **no magic** (every constant, path, and policy named and single-sourced), explicit and type-safe, small single-purpose units, with architecture invariants restored and every behavioral fix wrapped tests-first. This is a **plan only**: no source changes live here; each wave plan drives the implementation on its own branch.

## The five waves (shippable order)

Waves are ordered so each one lands on a foundation the previous wave already cleaned. Security and correctness ship first because every later refactor inherits those surfaces; the architecture-invariant fixes come next so the god-file splits carve already-correct dependency graphs; modularization, then duplication consolidation, then the broad consistency/cleanup pass land last.

### Wave 1 — Security & correctness · weight: **High**
[`wave-1-security-correctness.md`](./wave-1-security-correctness.md)

Closes every gap that lets a hostile repo, a synced config, or a misbehaving provider read secrets, weaken the sandbox, OOM the process, or silently corrupt recall: single-sourced credential-directory policy and an extended macOS deny-set, the `run_command` secrets boundary ratified in ADR 0009, the Anthropic stream byte-cap, model-scoped semantic recall, the typed sync trust gate (closing the `require→os` downgrade hole), bounded/atomic memory and session I/O, and the unmaintained-YAML migration. Hardening and correctness only — no feature work. It ships first because it de-risks all four later waves and depends on none of them.

### Wave 2 — Architecture-invariant fixes · weight: **High**
[`wave-2-architecture-invariants.md`](./wave-2-architecture-invariants.md)

Restores the modular-hexagonal invariants without changing behavior: the `Sandbox` port and the config writers stop leaking `anyhow` and return the typed `AgentError`; `shared/infra/config` stops depending into `modules/tools`; `SyncService` moves all filesystem work behind an adapter; credential resolution is single-sourced so boot and the live `/provider` swap can no longer drift; the TUI runtime stops being a second composition root; and the one deliberate domain↔framework coupling is ratified by ADR 0017. These are the dependency-graph corrections Wave 3's splits depend on.

### Wave 3 — Modularization of oversized files · weight: **High**
[`wave-3-modularization.md`](./wave-3-modularization.md)

Dissolves every oversized, multi-responsibility file into small single-purpose modules with no behavior change: the 2072-line `runtime.rs` god-file (and its ~410-line effect loop) behind a `RunLoop` state struct that removes all four `too_many_arguments` allows, plus `config.rs`, `keymap.rs`, `view_state.rs`, `markdown.rs`, the 247-line `AgentLoop::run`, the sync trust DTOs, and the oversized inline test modules. Each change is a pure move/structural split fully wrapped by the existing suite. It runs after Waves 1–2 so each god-file is opened once, already correct.

### Wave 4 — Duplication consolidation · weight: **Medium**
[`wave-4-duplication.md`](./wave-4-duplication.md)

Single-sources every block of logic copied across `src/`: the protocol-critical `Role`↔wire mapping, the SQLite blocking-store harness, the provider send/stream/body-read skeleton (where Wave 1's byte cap lands once for both adapters), the `now_rfc3339`/`write_atomic` primitives, the `<E: Display>→AgentError` mappers, the discard/collect `EventSink` doubles, the tool-args sanitizer, the open-coded `Notice` constructor, the `"function"` tool kind, and the `SYSTEM_PROMPT` facts that re-type live tool/limit/sensitive-pattern values (SEC-06: the prompt can no longer lie). Behavior-preserving except the named security hardenings.

### Wave 5 — Consistency, naming & dead-code cleanup · weight: **Medium / Low**
[`wave-5-consistency-cleanup.md`](./wave-5-consistency-cleanup.md)

The mechanical-hygiene catch-all that makes the whole tree read as one author: unify the module-file convention, the `Result` alias, and the `#[async_trait]` spelling; delete dead code and re-gate test-only helpers; trim the speculative full-CRUD memory ports (YAGNI); single-source scattered magic strings/numbers; align model-facing text on English; converge DTO/enum-conversion naming; add every missing error-justification comment; and remove the silent no-ops on user intent. It touches almost every file once, so it runs last to avoid re-churning files the earlier waves edited.

## Global coverage map

175 audit findings → 135 unique remediation items, each assigned to exactly one wave. The table lists every `primaryId`, its collapsed IDs, its wave, and its plan file.

### Wave 1 → `wave-1-security-correctness.md`

| primaryId | Collapsed IDs |
|---|---|
| SEC-01 | — |
| SEC-02 | — |
| SEC-03 | TOOL-04 |
| SEC-04 | SYNC-09 |
| SEC-07 | — |
| SYNC-01 | — |
| SYNC-02 | — |
| SYNC-03 | — |
| SYNC-08 | — |
| SYNC-11 | — |
| PROV-01 | — |
| PROV-04 | — |
| PROV-06 | — |
| PROV-08 | — |
| MEM-05 | — |
| MEM-07 | — |
| MEM-08 | — |
| MEM-10 | — |
| TOOL-06 | — |
| TOOL-07 | — |
| SESS-08 | — |
| SHARED-06 | — |
| SHARED-15 | — |
| BUILD-05 | — |

### Wave 2 → `wave-2-architecture-invariants.md`

| primaryId | Collapsed IDs |
|---|---|
| TOOL-01 | ARCH-01, ARCH-02 |
| SHARED-01 | ARCH-04 |
| SHARED-02 | — |
| ARCH-03 | SYNC-06 |
| ARCH-05 | ROOT-02, ERR-02 |
| ARCH-06 | ROOT-01, TUII-03 |
| TUIC-01 | — |

### Wave 3 → `wave-3-modularization.md`

| primaryId | Collapsed IDs |
|---|---|
| STRUCT-01 | TUII-01, BUILD-06, BUILD-04, TUII-05 |
| STRUCT-02 | — |
| STRUCT-03 | SHARED-03 |
| STRUCT-04 | TUIC-03 |
| STRUCT-05 | TUIC-02, TUIC-12 |
| STRUCT-10 | — |
| AGENT-01 | — |
| STRUCT-12 | SYNC-04 |
| STRUCT-07 | — |

### Wave 4 → `wave-4-duplication.md`

| primaryId | Collapsed IDs |
|---|---|
| DUP-07 | CONS-07 |
| DUP-04 | SESS-02 |
| DUP-05 | BUILD-03 |
| DUP-06 | SYNC-05 |
| DUP-01 | SESS-01 |
| DUP-02 | PROV-02, DUP-03 |
| DUP-08 | — |
| DUP-13 | — |
| TUII-02 | — |
| SHARED-04 | SEC-06 |

### Wave 5 → `wave-5-consistency-cleanup.md`

| primaryId | Collapsed IDs |
|---|---|
| CONS-01 | STRUCT-06 |
| CONS-02 | — |
| CONS-03 | DUP-10 |
| CONS-04 | — |
| CONS-05 | — |
| CONS-06 | MEM-11 |
| CONS-08 | — |
| AGENT-03 | CONS-09 |
| AGENT-02 | — |
| AGENT-04 | — |
| AGENT-05 | — |
| AGENT-06 | — |
| AGENT-07 | — |
| AGENT-08 | — |
| PROV-03 | — |
| PROV-05 | — |
| PROV-07 | — |
| PROV-10 | — |
| DUP-09 | — |
| DUP-11 | — |
| DUP-12 | PROV-09 |
| TOOL-02 | — |
| TOOL-03 | — |
| TOOL-05 | — |
| TOOL-10 | — |
| TOOL-11 | — |
| TOOL-12 | — |
| TUIC-04 | — |
| TUIC-05 | — |
| TUIC-06 | — |
| TUIC-07 | — |
| TUIC-08 | — |
| TUIC-09 | — |
| TUIC-10 | — |
| TUIC-11 | — |
| TUII-04 | — |
| TUII-06 | — |
| TUII-07 | ERR-04 |
| TUII-09 | — |
| TUII-10 | — |
| TUII-11 | — |
| TUII-12 | — |
| TUII-13 | — |
| MEM-01 | — |
| MEM-02 | BUILD-02 |
| MEM-03 | — |
| MEM-04 | — |
| MEM-06 | — |
| MEM-09 | — |
| MEM-12 | — |
| MEM-13 | — |
| MEM-14 | — |
| SESS-03 | — |
| SESS-04 | — |
| SESS-05 | — |
| SESS-06 | — |
| SESS-07 | — |
| SESS-09 | — |
| SESS-10 | — |
| SYNC-07 | — |
| SYNC-10 | — |
| SHARED-05 | — |
| SHARED-07 | — |
| SHARED-08 | — |
| SHARED-09 | — |
| SHARED-10 | — |
| SHARED-11 | — |
| SHARED-12 | — |
| SHARED-13 | — |
| SHARED-14 | — |
| ROOT-03 | — |
| ROOT-04 | — |
| ROOT-05 | — |
| ROOT-06 | — |
| ROOT-08 | — |
| ROOT-09 | — |
| ROOT-10 | — |
| ERR-01 | TUII-08 |
| ERR-03 | ROOT-07 |
| STRUCT-08 | — |
| STRUCT-09 | — |
| STRUCT-11 | — |
| STRUCT-13 | — |
| BUILD-01 | TOOL-08, TOOL-09 |
| BUILD-07 | — |

### Findings deliberately NOT assigned to a wave (non-actionable observations)

Two audit findings are intentionally outside the union of the wave plans because the audit itself concludes they require **no remediation**. They are flagged here for transparency, not as plan gaps:

| ID | Why it is not in any wave |
|---|---|
| SEC-05 | Inherent upstream `reqwest` limitation: API keys copied into `HeaderValue`/request buffers are not zeroized, with no in-codebase fix beyond keeping `expose()` call sites minimal (already done). Observation / known-residual only. |
| STRUCT-14 | The provider DTO/SSE files are test-heavy, **not** oversized; the audit explicitly recommends no production split. Its only actionable benefit (test-module extraction) is already covered by STRUCT-07 in Wave 3. |

All **135 actionable remediation items are assigned**; the only unassigned findings are these two non-actionable observations.

## Cross-wave dependency summary

The ordering is chosen so no wave re-touches what a later wave rewrites, and so security-sensitive ports stabilize before the structural waves rebuild on them.

- **Wave 1 → everything.** Wave 1 lands two **port signature changes** that later waves rebuild on: `Sandbox::command_policy` (TOOL-07) and `embedded_candidates(model, …)` (MEM-05). It also introduces the typed kernel `SandboxMode`/`NetworkStance` in `shared/kernel` (SYNC-02) — domain-safe, so when Wave 3 relocates the trust gate to `sync/domain` (STRUCT-12/SYNC-04) it stays pure. The shared `MAX_STREAM_BYTES`/`bounded_preview` (PROV-01/PROV-06) must survive into Wave 4's provider-skeleton dedup rather than be re-duplicated. Wave 1 waits for nothing.
- **Wave 2 → Wave 3.** The god-file splits (config, runtime, view_state) must run **after** the invariant fixes so they split files whose imports, error types, and composition are already correct: SHARED-02 (config writers → `AgentError`) and SHARED-01 (lift `tools` types out of config) must precede the `config.rs` split (STRUCT-03); ARCH-06/TUII-03 (inject `SyncService`, kill the hardcoded DB path) must precede `runtime/sync.rs` (STRUCT-01). The ADR 0017 guard test from TUIC-01 must be repointed when STRUCT-05 moves `InputBuffer` into `input_buffer.rs`.
- **Wave 2/4 alignment.** Wave 4's typed `AgentError::memory/session` constructors (DUP-07) are what SHARED-02's writer change consumes; Wave 4's `persist_or_notice` (TUII-02) is deliberately generic over `E: Display` so it survives that change; `render_system_prompt` (SHARED-04) takes its values as parameters so it never deepens SHARED-01.
- **Wave 4 internal + Wave 1 merge.** Wave 4 is sequenced primitives-first (typed constructors → role mapping → `"function"` const → time/fs helpers → SQLite harness → sinks/tool-args → shared provider loop) so every consumer is edited only after its single source exists. PROV-01's cap is **co-owned with Wave 1**: coordinate so exactly one ceiling exists on the Anthropic stream (Wave 4 replaces any inline Wave 1 cap with the shared `enforce_stream_budget`).
- **Wave 5 ↔ all.** Wave 5 shares `app::wire` with Wave 2 (rewrite the function once), shares the memory/session SQLite adapters with Waves 1 and 4, and shares the provider adapters with Waves 1 and 4. STRUCT-08 (group `tui/infrastructure`) must follow Wave 3's runtime split. Several Wave 5 steps set conventions other waves adopt (the `AgentResult<T>` alias, the `as_wire`/`FromStr` enum shape, the single tool timeout/cap constants).

This order de-risks by shrinking the surface at each green checkpoint: hostile-input surfaces close first, the dependency graph straightens second, files shrink to single-purpose third, copies collapse fourth, and the broad naming/hygiene pass lands on an already-correct tree last.

## Branch & PR strategy

- **Never commit to `main`** (protected; remote `origin` = github.com/TheoOdawara/Kiri). One **feature branch per wave**: `fix/wave-1-security-correctness`, `fix/wave-2-architecture-invariants`, `fix/wave-3-modularization`, `fix/wave-4-duplication`, `fix/wave-5-consistency-cleanup`.
- **Conventional Commits**, English, imperative subject; the body explains the *why*. Commit per coherent step within a wave (the plans are step-ordered for exactly this). Show the diff and get approval before each commit; open one PR per wave.
- Deferred security items become **GitHub issues labeled `security-debt`** via `gh` (e.g. SEC-07's env-key-import opt-out, BUILD-05's YAML-vs-TOML format decision), never dropped silently.

## Global definition-of-done gate

Every step, and every wave before its PR, must pass the project gate — **each command exit 0, in order**:

```
cargo fmt --check  →  cargo clippy --all-targets -- -D warnings  →  cargo build  →  cargo test
```

Plus, per the contract: performance sanity on hot paths, **run-to-verify** (launch the TUI / run the headless path and observe the touched behavior), and security accounted for (fixed or filed as `security-debt`). Fix every error in touched files, including pre-existing ones.

## How to execute

1. **One wave per branch, in order.** Do not start a wave until the prior wave is merged green — the cross-wave dependencies above assume it.
2. **Tests-first, per step.** Each behavioral fix gets its test contract written before the code (behavior happy + edges, error modes, security cases, regression lock). Pure moves/renames are locked by the existing suite plus a before/after `cargo test -- --list` diff; cosmetic/doc-only changes are exempt.
3. **Conform to the architecture strictly.** Ports return `AgentError`; `anyhow` only at the binary edge; network I/O only in `provider`/`sync` infrastructure; filesystem I/O only at the `FsSandbox` chokepoint (and the data-dir-owning contexts); `domain` stays pure. Any deviation needs an ADR (the plans call out exactly which: 0009, 0017, and amendments to 0003/0007/0010/0013/0014/0015).
4. **Re-run the audited scope after each wave.** After a wave merges, re-verify the surfaces it touched (the per-wave Definition of Done lists the concrete grep/test/run checks) before opening the next wave's branch, so regressions surface immediately rather than compounding.