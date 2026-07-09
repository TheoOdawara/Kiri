# Remediation Worklist — Full Sweep 2026-07-09

**Epic:** https://github.com/TheoOdawara/Kiri/issues/78  
**Report:** [`2026-07-09-full-sweep.md`](./2026-07-09-full-sweep.md)

Integration branch suggestion: `fix/full-sweep-remediation` off `main`  
Gate every commit: `fmt → clippy -D warnings → build → test`

---

## Batch 0 — Docs (standalone)

| Step | Issue | Files |
|------|-------|-------|
| R0.1 | **#85** README ADR 0020 | `README.md` |
| R0.2 | **#94** ProviderSwap contract | `CLAUDE.md` |
| R0.3 | **#99** ADR 0003 note | `docs/decisions/0003-…` |
| R0.4 | **#126** Windows key notes | `README.md` |
| R0.5 | **#117** renumber ADRs 0018/0019 | `docs/decisions/*` + links |

---

## Batch 1 — P0 Critical/High (parallel if file-disjoint)

| Step | Issue | Finding | Notes |
|------|-------|---------|-------|
| R1.1 | **#79** | F-SEC-001 | Deny paths under `kiri_home()` in resolve_* |
| R1.2 | **#80** | F-BUG-001 | EventStream None/Err ≠ Idle in turn/distill |
| R1.3 | **#42** | F-BUG-002 | Stream pipe cap + process tree (pre-existing) |
| R1.4 | **#82** + **#62** | F-SEC-003 / output | `confirm_in_auto` + result cap (chain MCP files) |
| R1.5 | **#81** | F-SEC-002 | Expand SECRET_DIRS / sensitive |
| R1.6 | **#83** | F-PERF-001 | Cap edit_diff before TextDiff |
| R1.7 | **#84** | F-PERF-002 | Cap stream_landings |
| R1.8 | **#86** | F-UX-002 | Unconfigured gate on custom slash |

Safe parallel example: R1.1 ‖ R1.2 ‖ R1.3 ‖ R1.5 ‖ R1.6 ‖ R1.7 ‖ R1.8; R1.4 chained.

---

## Batch 2 — OpenFile / Windows honesty / sensitive

| Step | Issue | Notes |
|------|-------|-------|
| R2.1 | **#88** + **#59** + **#116** + **#93** | OpenFile resolve + env scrub + editor default + document spawn |
| R2.2 | **#87** | Sensitive empty fail-closed |
| R2.3 | **#89** | require gates hooks |
| R2.4 | **#112** + **#90** | BootNotice no OS confine; Windows residual |
| R2.5 | **#100** | edit_file capped read |
| R2.6 | **#109–#111, #113–#115** | UX feedback batch (wizard/mode/busy/provider/status) |

---

## Batch 3 — Known-open security + guards

| Step | Issue |
|------|-------|
| R3.1 | **#61** approval backticks |
| R3.2 | **#60** blank key |
| R3.3 | **#57** instructions open nofollow |
| R3.4 | **#58** 3xx error string |
| R3.5 | **#97** inward-import guard |
| R3.6 | **#98** domain I/O needles |
| R3.7 | **#124** process-spawn allowlist |
| R3.8 | **#122** cargo-audit CI |
| R3.9 | **#91** FileSecretStore lock |
| R3.10 | **#101** resolve_create race harden |

---

## Batch 4 — Perf / polish / product

| Step | Issue |
|------|-------|
| R4.1 | **#108** / **#44** markdown cache |
| R4.2 | **#103** conversation clone / compaction ADR |
| R4.3 | **#104** MCP schema bytes |
| R4.4 | **#105–#107** memory/transcript |
| R4.5 | **#43** /sync non-blocking |
| R4.6 | **#118** split runtime/app |
| R4.7 | **#119** dead ponytail |
| R4.8 | **#102** tool args empty object |
| R4.9 | **#125** badge i18n |
| R4.10 | **#114** CLI seed notice |
| R4.11 | **#120** Pre/PostToolUse (feature ADR) |
| R4.12 | **#121** skill script (feature or drop field) |
| R4.13 | **#45** plaintext creds (product) |
| R4.14 | **#92** hash length (optional) |
| R4.15 | **#96** CommandSandbox type (ADR accept or opaque type) |
| R4.16 | **#123** tokio timeout residual (document/measure) |
| R4.17 | **#95** anyhow placement |

---

## Definition of done (remediation)

1. All **Batch 1** issues closed or explicitly deferred with comment on epic.  
2. Gate green every commit.  
3. Completeness critic: every finding in the report maps to an issue (done) and every P0 is fixed or still open with owner.  
4. Run-to-verify Criticals #79 and #80.  
5. Security-auditor on security batches; never commit to `main` directly.
