# Full Sweep Audit — 2026-07-09

Branch: `audit/full-sweep` · Product: Kiri v0.1.0  
**Epic:** https://github.com/TheoOdawara/Kiri/issues/78  

Axes: security · architecture · bugs · performance · UX · UI · maintainability

---

## Executive summary (pt-BR)

Varredura multi-eixo (~41k LOC, **1025 testes verdes**). Base de segurança e hexágono estão maduros; a varredura unificou findings novos + dívida já aberta + residuals que antes ficavam só em ADR.

### Top riscos (P0)

| # | Issue | Finding | Sev | Resumo |
|---|-------|---------|-----|--------|
| 1 | **#79** | F-SEC-001 | Critical | File tools não negam `~/.kiri`; com workspace em `$HOME` + auto, dá para reescrever config / plantar hooks e exfiltrar API key |
| 2 | **#80** | F-BUG-001 | Critical | `drive_turn`/`distill`: EventStream fechado → `Idle` sob `select! biased` → hang/spin |
| 3 | **#42** | F-BUG-002 | High | Pipes de exec sem cap em streaming (issue pré-existente) |
| 4 | **#82** | F-SEC-003 | High | MCP em Auto sem `confirm_in_auto` + output (#62) |
| 5 | **#81** | F-SEC-002 | High | SECRET_DIRS incompleto (cloud CLIs) |
| 6 | **#83/#84** | F-PERF-001/002 | High | Diff LCS sem cap; `stream_landings` por newline |
| 7 | **#85/#86** | F-UX-001/002 | High | README keyring drift; slash custom bypassa onboarding |

### O que está sólido

- Gate: `fmt` / `clippy -D warnings` / `build` / **1025** tests — exit 0  
- Env scrub em `run_shell` (ADR 0026; #49/#25 fechados e verificados)  
- HTTP redirects desabilitados; caps de stream (ADR 0025)  
- Project config só `effort`; `.env` só `~/.kiri/`  
- Domain purity + hooks/mcp process confinement guards  
- Onboarding first-run; first frame antes de SessionStart hooks  
- Provider SSE, session SQLite, distiller bounds  

---

## Baseline (Wave 0)

| Check | Result |
|-------|--------|
| `cargo fmt --check` | exit **0** |
| `cargo clippy --all-targets -- -D warnings` | exit **0** |
| `cargo build` | exit **0** |
| `cargo test` | **1025** passed, 0 failed, 1 ignored |
| `cargo audit` | **not installed** → **#122** |

---

## Known debt reconciliation

### Pre-existing open (linked to epic #78; no duplicate)

| Issue | Maps to |
|-------|---------|
| **#62** | F-SEC-009, F-BUG-004 MCP output |
| **#61** | F-SEC-012 approval markdown |
| **#60** | F-SEC-011 blank key |
| **#59** | F-SEC-008 git/EDITOR env |
| **#58** | F-SEC-014 3xx UX |
| **#57** | F-SEC-010 instructions TOCTOU |
| **#45** | F-SEC-013 plaintext creds |
| **#42** | F-BUG-002 stream/pipe cap + process tree |
| **#44** | related F-PERF-003 / **#108** |
| **#43** | sync push blocks event loop |

### Closed / verified this sweep

| Item | Note |
|------|------|
| #49 / #25 env scrub | `scrub_env` in `exec.rs` verified |
| #26 NTFS ADS | blocked in sandbox |
| #24 redirects | `Policy::none()` verified |
| #53 tokio cancel | residual re-opened as **#123** (user asked to track accepts) |
| #51 first frame | verified + test |

### Ponytail harvest → issues

| Marker | Issue |
|--------|-------|
| Pre/PostToolUse no dispatch | **#120** |
| skill script never run | **#121** |
| dead frontmatter accessors / catalog.resources | **#119** |
| sandbox sync path no timeout | documented in #79/#101 context + ponytail on sandbox.rs |
| MCP Http unstarted | product (stdio only ADR 0021) — tracked via epic notes |

---

## Scoreboard (new + residual issues filed this sweep)

| Axis | New issues | Pre-existing linked |
|------|------------|---------------------|
| Security | #79–#82, #87–#92, #123 | #45, #57–#62 |
| Architecture | #93–#99, #124 | — |
| Bugs | #80, #100–#102 | #42 |
| Performance | #83–#84, #103–#108 | #43–#44 |
| UX/UI | #85–#86, #109–#116, #125–#126 | — |
| Maintainability / CI / product gaps | #117–#122 | — |

Rough severity of **new** findings (not counting pre-existing):

| Sev | Count (approx) |
|-----|----------------|
| Critical | 2 (#79, #80) |
| High | 6 |
| Medium | ~25 |
| Low/Info | ~15 |

---

## Findings registry (with GitHub issues)

### Security

| ID | Sev | Issue | Status |
|----|-----|-------|--------|
| F-SEC-001 | Critical | **#79** | new · fix-now |
| F-SEC-002 | High | **#81** | new |
| F-SEC-003 | High | **#82** | new (+ #62) |
| F-SEC-004 | Medium | **#87** | new |
| F-SEC-005 / F-ARCH-001 | High/Med | **#88** | new (+ #59 env) |
| F-SEC-006 | Medium | **#89** | new |
| F-SEC-007 | Medium | **#90** | residual Windows jail |
| F-SEC-008 | Medium | **#59** | known-open |
| F-SEC-009 | Medium | **#62** | known-open |
| F-SEC-010 | Medium | **#57** | known-open |
| F-SEC-011 | Medium | **#60** | known-open |
| F-SEC-012 | Medium | **#61** | known-open |
| F-SEC-013 | Medium | **#45** | known-open |
| F-SEC-014 | Low | **#58** | known-open |
| F-SEC-015 | Low | **#91** | new |
| F-SEC-016 | Info | **#92** | residual hash trunc |
| ADR 0024 residual | Info | **#123** | re-tracked accept |
| Env scrub / ADS / redirects / SSE | — | accept verified | clean |

### Architecture

| ID | Sev | Issue |
|----|-----|-------|
| F-ARCH-001 | High | **#88** (same OpenFile) |
| F-ARCH-002 | Medium | **#93** |
| F-ARCH-003 | Medium | **#94** |
| F-ARCH-004 | Medium | **#95** |
| F-ARCH-005 | Low | **#96** |
| F-ARCH-006 | Low | **#97** |
| F-ARCH-007 | Low | **#98** |
| F-ARCH-008 | Info | **#99** |
| Spawn allowlist guard | Medium | **#124** |

### Bugs

| ID | Sev | Issue |
|----|-----|-------|
| F-BUG-001 | Critical | **#80** |
| F-BUG-002 | High | **#42** (pre-existing) |
| F-BUG-003 | Medium | **#100** |
| F-BUG-004 | Medium | **#62** |
| F-BUG-005 | Medium | **#101** |
| F-BUG-006 | Low | **#102** |

### Performance

| ID | Sev | Issue |
|----|-----|-------|
| F-PERF-001 | High | **#83** |
| F-PERF-002 | High | **#84** |
| F-PERF-003 | Medium | **#108** (+ #44) |
| F-PERF-004 | Medium | **#103** |
| F-PERF-005 | Medium | **#104** |
| F-PERF-006 | Medium | **#105** |
| F-PERF-007 | Low | **#106** |
| F-PERF-008 | Low | **#107** |
| Sync block | — | **#43** |

### UX / UI

| ID | Sev | Issue |
|----|-----|-------|
| F-UX-001 / F-MAINT-001 | High | **#85** |
| F-UX-002 | High | **#86** |
| F-UX-003 | Medium | **#109** |
| F-UX-004 | Medium | **#110** |
| F-UX-005 | Medium | **#111** |
| F-UX-006 | Medium | **#112** |
| F-UX-007 | Medium | **#113** |
| F-UX-008 | Low | **#114** |
| F-UI-001 | Medium | **#115** |
| F-UI-002 | Low | **#125** |
| F-UI-003 | Medium | **#116** |
| F-UI-006 | Low | **#126** |

### Maintainability / product

| ID | Sev | Issue |
|----|-----|-------|
| F-MAINT-002 ADR numbers | Medium | **#117** |
| F-MAINT-004 god files | Medium | **#118** |
| F-MAINT-005 dead code | Low | **#119** |
| Pre/PostToolUse | — | **#120** |
| Skill script | — | **#121** |
| cargo-audit CI | — | **#122** |

---

## Module coverage

| Module | Status |
|--------|--------|
| tools | findings #79 #81 #87 #89 #90 #100 #101 #42 |
| provider | clean controls; #45 #58 #60 #91 #102 |
| extensions/hooks/mcp | #82 #62 #89 #92 #104 #120 #121 |
| agent | #103; task depth OK |
| memory | #105 #107 |
| session | scanned OK |
| sync | #59 #43 |
| tui | #80 #83 #84 #88 #93 #108–#116 #125 |
| shared/config | #57 #85 |
| app/main | #94 #95 #118 #122 |
| architecture_guards | #97 #98 #124 |

---

## Method

- Wave 0 baseline + Known Debt (`gh` security-debt + ADRs + ponytail)  
- Parallel read-only agents: security-auditor, architecture-explorer, bugs/perf, UX/UI/maint  
- Criticals re-checked by orchestrator against source (`secret_paths.rs`, `turn.rs`)  
- Pre-existing issues commented + labeled `audit-2026-07`  
- Accept residuals filed as issues per user request (#90, #92, #96, #123, …)  

Remediation order: [`2026-07-09-remediation-worklist.md`](./2026-07-09-remediation-worklist.md)
