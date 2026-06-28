# Kiri — Code Sweep (scan phase)

> Date: 2026-06-27
> Pass: read-only multi-agent audit — scan phase only (findings, no code changed)

This is the master index for a read-only quality + security sweep of the Kiri crate. **17 auditor agents** each owned exactly one findings file (10 per-module, 7 cross-cutting), reading their scope in full and cross-checking callers via grep. No source was modified; the only writes in this phase are the findings files under `docs/audit/` and this index. Cross-cutting agents (architecture invariants, duplication, security, error-handling, structure, consistency, build hygiene) re-examined the whole graph to catch what no single module pass can see — inverted dependencies, repeated logic across modules, and asymmetric security coverage. The result is **175 findings, zero Critical**: the architecture is real and disciplined, error handling and secret handling are strong, and the debt is concentrated in a small number of high-leverage spots (one god-file, one anyhow port, one bypassed security chokepoint, and a cluster of cross-module duplication).

## Severity rollup

| Area | Critical | High | Medium | Low | Total |
|---|---|---|---|---|---|
| agent module | 0 | 1 | 2 | 5 | 8 |
| provider module | 0 | 1 | 3 | 6 | 10 |
| tools module | 0 | 1 | 5 | 6 | 12 |
| tui core (domain + application) | 0 | 1 | 7 | 4 | 12 |
| tui infrastructure | 0 | 1 | 5 | 7 | 13 |
| memory module | 0 | 0 | 5 | 9 | 14 |
| session module | 0 | 0 | 2 | 8 | 10 |
| sync module | 0 | 1 | 5 | 5 | 11 |
| shared kernel + infra | 0 | 1 | 6 | 8 | 15 |
| composition root + entrypoints | 0 | 0 | 6 | 4 | 10 |
| architecture & layer invariants | 0 | 1 | 4 | 1 | 6 |
| cross-module duplication | 0 | 2 | 5 | 6 | 13 |
| security sweep | 0 | 1 | 2 | 4 | 7 |
| error-handling compliance | 0 | 0 | 0 | 4 | 4 |
| file size & folder organization | 0 | 2 | 7 | 5 | 14 |
| naming & convention consistency | 0 | 0 | 5 | 4 | 9 |
| build hygiene (clippy/dead-code/deps) | 0 | 0 | 5 | 2 | 7 |
| **TOTAL** | **0** | **13** | **74** | **88** | **175** |

> Note: the High/Medium counts include duplicate views of the same underlying issue (e.g. the Sandbox `anyhow` port is counted by both the tools pass and the architecture pass). See [Cross-references & overlaps](#cross-references--overlaps) so remediation does not double-count.

## Top findings (highest impact first)

Highest blast radius and security first, then architecture invariants, oversized files, and cross-module duplication. Near-duplicate findings are collapsed with their linked IDs noted.

1. **SEC-01** — `run_command` bypasses the sensitive-file / secret-dir chokepoint; only the OS sandbox guards it — **High** — `tools/infrastructure/fs/run_command.rs:148-186`, `exec.rs:90-113`, `confine/noop.rs:10-22` — file tools refuse `.env`/`~/.aws`, but `run_command cat .env` reads exactly that and returns it to the model; on Linux/Windows the OS sandbox is a pass-through, so zero secret protection (mitigated only by user confirmation).
2. **SYNC-01** — Trust gate misses the `sandbox.mode = require → os` downgrade — **High** — `sync/application/sync_service.rs:349-353` — `risky_config_changes` only flags `→ off`, so a synced config relaxing a hardened `require` to `os` weakens the sandbox with no `--force` prompt; a hole in the security control itself.
3. **PROV-01** — Anthropic stream accumulator lacks the unbounded-response byte cap the OpenAI path enforces — **High** — `provider/infrastructure/anthropic/sse.rs:81` (vs `openai/sse.rs:17,46`) — a misbehaving/compromised Messages endpoint that streams forever can OOM the process; `read_timeout` only bounds idle time between chunks. See also DUP-03.
4. **TOOL-01 / ARCH-01** — `Sandbox` application port returns `anyhow::Result` instead of `AgentError` — **High** — `tools/application/sandbox.rs:3,34,38` — a direct invariant violation on the central filesystem-chokepoint port, inconsistent with its sibling `CommandSandbox` (returns `AgentError`) in the same folder; functionally contained today only because tools stringify the error. See also ARCH-02.
5. **SHARED-01 / ARCH-04** — `shared/infra/config` depends *into* `modules/tools` (application **and** infrastructure) — **High** — `shared/infra/config.rs:11-12` — the one shared file that does `use crate::modules::…`, calling a tools *adapter* constructor (`load_sensitive_matcher`), inverting the "shared is the leaf" dependency direction and propagating tools' `anyhow` into `Settings`.
6. **TUIC-01** — Domain layer depends on `ratatui` and `tui_textarea` rendering crates — **High** — `tui/domain/view_state.rs:1-2,22-24,133,141` — the only domain file importing UI framework crates; `InputBuffer` embeds a `TextArea`, breaching "domain = pure data, no frameworks." A documented tradeoff worth ratifying via ADR rather than a silent breach.
7. **STRUCT-01 / TUII-01** — Split the `runtime.rs` god-file by responsibility — **High** — `tui/infrastructure/runtime.rs` (2072 lines, ~1627 prod) — the only true god-file, fusing six concerns (provider swap, event loop, session ops, distillation, sync push, turn driver). See also BUILD-04/BUILD-06, TUII-05.
8. **STRUCT-02** — Extract the ~410-line `Tui::run` event loop into per-effect handlers — **High** — `tui/infrastructure/runtime.rs:301-710` — the largest single function in the crate, an 18-arm inline `match effect` mixing 30-60-line inline arms with delegated ones; untestable as a unit.
9. **AGENT-01** — Split the 247-line `run` method; extract per-call decision and checkpoint handling — **High** — `agent/application/agent_loop.rs:111-358` — the core agent loop carries per-call decisioning, checkpoint reset, and tool dispatch in one method.
10. **DUP-01 / SESS-01** — Unify the SQLite blocking-store harness shared by `memory` and `session` — **High** — `memory/infrastructure/sqlite_shared_memory.rs:116-138`; `session/infrastructure/sqlite_session_store.rs:72-93` — the single largest cross-module copy: `run_blocking`/`lock`/`DB_OP_TIMEOUT`/error-mapper/open-with-parent, differing only by the `AgentError` variant.
11. **DUP-04 / SESS-02** — Collapse the three copies of the canonical `Role`↔wire-string mapping — **High** — `provider/infrastructure/openai/message_dto.rs:105-112`; `session/infrastructure/message_dto.rs:23-42` — the protocol detail most dangerous to drift: the session store persists these strings and the provider sends them; one renamed variant and stored vs sent history disagree silently. See also CONS-05/CONS-06.
12. **SEC-02** — OS read-deny list omits Kiri's own `~/.kiri/credentials.json` and common home credential files — **Medium** — `tools/infrastructure/confine/macos.rs:13,86-122`; `shared/infra/config.rs:744` — a confined `run_command` can `cat ~/.kiri/credentials.json` (the 0600 key fallback store); the file tools *do* block it, making this an inconsistency. See also SEC-01, SEC-03.
13. **SEC-03 / TOOL-04** — Credential-directory list duplicated across `sandbox.rs` and `confine/macos.rs` and drifted from the sensitive-name list — **Medium** — `tools/infrastructure/sandbox.rs:19`; `confine/macos.rs:13`; `sensitive.rs:10-40` — one security policy enforced in two layers with nothing tying them together; the root cause that makes SEC-02 easy to introduce.
14. **SYNC-03** — `import` reads the whole untrusted NDJSON into memory before the entry cap applies — **Medium** — `sync/infrastructure/memory_ndjson.rs:54,64-66` — `IMPORT_CAP` bounds entries processed, but `fs::read_to_string` slurps a multi-GB hostile remote file first; the cap does not prevent the memory-exhaustion DoS it documents.
15. **SHARED-15** — Private-dir hardening applied inconsistently and its failure swallowed with an inaccurate justification — **Medium** — `shared/infra/config.rs:677,305,368` — the `0700` hardening on the data dir is uneven and its error is dropped with a comment that does not match what is ignored.
16. **MEM-05** — Embedding model/dim metadata stored but never consulted at recall — model switches silently degrade recall — **Medium** — `memory/infrastructure/sqlite_shared_memory.rs:75` — vectors from a previous embedding model are mixed with the new model's at recall, silently corrupting semantic relevance with no error.
17. **ARCH-03 / SYNC-06** — `sync` application service performs filesystem I/O inline instead of behind an adapter — **Medium** — `sync/application/sync_service.rs:200-205,239-248` — git and ndjson are behind ports, but the work-tree/config fs work (`write_atomic`, copy, read) is raw `tokio::fs` in the use-case layer. See also DUP-06.
18. **ARCH-05 / ROOT-02** — Credential-resolution logic duplicated across `app.rs` and the TUI `ProviderSwap` — **Medium** — `app.rs:390-424`; `tui/infrastructure/runtime.rs:144-167` — a security-sensitive rule (keyless short-circuit → stored secret → env import → persist) written twice and already diverging. See also ARCH-06, ERR-02.
19. **SHARED-02** — Config writers return `anyhow::Result` and are called from the live TUI runtime (not the binary edge) — **Medium** — `shared/infra/config.rs:378-404` → `runtime.rs:977,1015,1053,1127` — `anyhow::Error` flows back into runtime-reachable `/models`/`/effort`/`/provider` handlers, diverging from "ports return `AgentError`."
20. **BUILD-05** — `serde_yaml` dependency is unmaintained/deprecated (`0.9.34+deprecated`) — **Medium** — `Cargo.toml:18`, sole use `memory/infrastructure/file_project_memory.rs:181,191` — RUSTSEC-flagged, parses attacker-influenceable memory front-matter; track as `security-debt`, migrate to a maintained crate or TOML front-matter.
21. **DUP-02 / PROV-02** — Extract the shared provider stream-and-status loop — **Medium** — `openai/provider.rs:87-117`; `anthropic/provider.rs:91-118`; `openai/embeddings.rs:96-103` — the `<error body unavailable>` read + classify and the eventsource drain loop are copied across all three HTTP adapters; centralizing them is where PROV-01's byte cap should land once.
22. **TUII-03** — `sync_push` constructs sibling-context adapters inline and hardcodes the memory DB path — **Medium** — `tui/infrastructure/runtime.rs:1367-1390` — the runtime instantiates `SqliteSharedMemory`/`GitCli`/`SyncService` and hardcodes a data path, a layer/coupling leak that makes the runtime a second composition root. See also ARCH-06.
23. **SHARED-04 / SEC-06** — `SYSTEM_PROMPT` hardcodes tool inventory/limits/sensitive patterns that authoritatively live in code — **Medium** — `shared/infra/config.rs:44,61,92,107` — the prompt re-types the tool count, the 30s/64 KiB limits, and the sensitive-pattern list; a `KIRI_SENSITIVE_PATTERNS` override makes the prompt lie to the model with no compile signal.
24. **SYNC-02** — Trust gate keyed to hand-typed magic strings decoupled from the real enums — **Medium** — `sync/application/sync_service.rs:323,349,354-360` — the security gate compares against bare `"none"`/`"off"`/`"allow"` literals; if `AuthMethod`'s wire spelling changes, the gate silently stops detecting downgrades with no test failure.
25. **TOOL-06** — `edit_file` performs un-timed blocking `std::fs` I/O on the async runtime — **Medium** — `tools/infrastructure/fs/edit_file.rs:79,92` — the one error-handling gap: blocking read/write on the single-threaded runtime against the "all I/O has a timeout" rule.
26. **STRUCT-03 / SHARED-03** — Split `config.rs` — Raw DTOs / resolution / writers / CLI / Settings are five concerns — **Medium** — `shared/infra/config.rs` (~827 prod lines) — five reasons-to-change in one god-file (incl. a 100-line system prompt literal). See also BUILD-06.
27. **CONS-01 / STRUCT-06** — Module-file convention split: `mod.rs` (memory/session/sync) vs `<name>.rs` everywhere else — **Medium** — `modules/{memory,session,sync}/**/mod.rs` vs `modules/{agent,provider,tools,tui}.rs` — the most visible cross-cutting drift; a reader cannot predict where a module's submodule list lives.
28. **BUILD-02 / MEM-02** — Speculative future-UI port surface retained behind `#[allow(dead_code)]`, including a duplicated double-port for memory — **Medium** — `memory/application/project_memory.rs:12` (+ `shared_memory`/`project_store`/`shared_store`) — two parallel port tiers for one concept and a large unused CRUD surface kept compiling by allow attributes (YAGNI).

## Cross-references & overlaps

Where two (or more) auditors flagged the same underlying issue from different angles, remediate once against the linked set:

- **Sandbox `anyhow` port** — `TOOL-01` ≡ `ARCH-01`; the spread into adapters is `ARCH-02`. One fix.
- **`runtime.rs` god-file** — `STRUCT-01` ≡ `TUII-01`; also surfaced by `BUILD-06` (size) and `BUILD-04`/`TUII-05` (the four `too_many_arguments` allows are a symptom).
- **`config.rs` god-file** — `SHARED-03` ≡ `STRUCT-03`; also in `BUILD-06`.
- **config → tools dependency inversion** — `SHARED-01` ≡ `ARCH-04`.
- **SQLite blocking-store harness duplication** — `DUP-01` ≡ `SESS-01` (memory + session).
- **`Role`↔wire-string mapping (3 copies)** — `DUP-04` ≡ `SESS-02`; related naming angles `CONS-05` (DTO names) and `CONS-06` (four conversion shapes).
- **`now_rfc3339()` duplication** — `DUP-05` ≡ `BUILD-03` (memory `entry.rs` + session store); the symmetric RFC3339 *parse* is `SYNC-08`/`merge.rs`.
- **atomic-write helper duplication** — `DUP-06` ≡ `SYNC-05` (memory + sync).
- **per-module `<E: Display>→AgentError` mappers** — `DUP-07` ≡ `CONS-07` (the `mem`/`sess`/`ser` abbreviations).
- **credential-directory list duplication / drift** — `SEC-03` ≡ `TOOL-04`; the coverage gap it enables is `SEC-02`.
- **sync application-layer fs I/O** — `ARCH-03` ≡ `SYNC-06`.
- **sync trust DTOs** — `SYNC-04` (belongs in `domain/`) and `STRUCT-12` (extract to its own file) are the same code from architecture vs file-size angles.
- **credential-resolution duplication** — `ARCH-05` ≡ `ROOT-02`; the silent-vs-surfaced persist divergence it causes is `ERR-02`; the broader "second composition root" framing is `ARCH-06`.
- **`canonicalize()` degradation, undocumented, ×3** — `ERR-03` ≡ `ROOT-07`.
- **provider HTTP loop / accumulator duplication** — `DUP-02` ≡ `PROV-02`; the parallel `TurnAccumulator`s are `DUP-03`; the missing byte cap that should land in the shared loop is `PROV-01`.
- **system-prompt value drift** — `SHARED-04` ≡ `SEC-06` (the sensitive-pattern subset).
- **`let _ = draw_and_copy` missing justification** — `ERR-01` ≡ `TUII-08`.
- **markdown `start(blocks)` unused param** — `ERR-04` ≡ `TUII-07`.
- **dead-code `#[allow]` inventory** — `BUILD-01`/`BUILD-02` overlap the per-module stale-dead-code findings `MEM-02`, `PROV-03`, `SESS-03`, `TOOL-08`, `TOOL-09`.
- **module-file convention** — `CONS-01` ≡ `STRUCT-06`.
- **`view_state.rs` grab-bag** — `TUIC-02` ≡ `STRUCT-05`.
- **`keymap.rs` split** — `TUIC-03` ≡ `STRUCT-04`.
- **`markdown.rs` split** — `STRUCT-10` (with the `TUII-12` cache-clone perf nit in the same file).
- **canonical `"function"` tool-kind literal** — `DUP-12` ≡ `PROV-09`.
- **`pt-BR` vs English model-facing tool results** — `CONS-09` ≡ `AGENT-03` (the intra-file language mix).

## Suggested remediation sequencing

Themes only — concrete edits belong to the remediation phase. Each wave is independently shippable and ordered so earlier waves de-risk later ones.

1. **Security & correctness** — close the chokepoint gaps and the one recall bug first: `SEC-01`/`SEC-02`/`SEC-03`+`TOOL-04` (run_command + OS deny-list + single-source the credential-dir list), `SYNC-01`/`SYNC-02`/`SYNC-03` (trust-gate downgrade hole, stringly-typed gate, unbounded import read), `PROV-01` (Anthropic byte cap), `SHARED-15` (private-dir hardening), `MEM-05` (embedding model/dim at recall), `TOOL-06` (un-timed `edit_file` I/O), and `BUILD-05` (deprecated `serde_yaml` → `security-debt`).
2. **Architecture-invariant fixes** — restore the layer rules: `TOOL-01`/`ARCH-01`+`ARCH-02` (Sandbox port + tools adapters → `AgentError`), `SHARED-01`/`ARCH-04` (config stops depending into tools), `SHARED-02` (config writers → `AgentError`), `ARCH-03`/`SYNC-06` (sync fs behind an adapter), `ARCH-05`/`ROOT-02`+`ARCH-06`+`TUII-03` (single credential-resolution + tame the runtime second-root), and ratify `TUIC-01` via ADR.
3. **Modularization of oversized files** — dissolve the god-files once their dependencies are clean: `STRUCT-01`/`TUII-01` + `STRUCT-02` (runtime + `Tui::run`), `STRUCT-03`/`SHARED-03` (config), `TUIC-02`/`STRUCT-05` (view_state), `TUIC-03`/`STRUCT-04` (keymap), `STRUCT-10` (markdown), `AGENT-01` (agent `run`), `STRUCT-12` (sync trust DTOs), and `STRUCT-07` (extract oversized inline `#[cfg(test)]` modules).
4. **Duplication consolidation** — single-source the repeated logic: `DUP-01`/`SESS-01` (SQLite harness), `DUP-04`/`SESS-02` (Role wire mapping), `DUP-02`/`PROV-02`+`DUP-03` (provider loops/accumulators), `DUP-05`/`BUILD-03` (`now_rfc3339`), `DUP-06`/`SYNC-05` (atomic write), `DUP-07` (error mappers), `DUP-08` (EventSink doubles), `DUP-13` (JSON-args normalizers), `TUII-02` (Notice helper), and `SHARED-04`/`SEC-06` (generate the prompt fragment from the live matcher).
5. **Consistency, naming & dead-code cleanup** — the mechanical pass: `CONS-01`/`STRUCT-06` (module-file convention), `CONS-02` (`async_trait` spelling), `CONS-03` (`Result` alias), `CONS-05` (DTO naming), `CONS-09`/`AGENT-03` (model-facing language), `BUILD-01`/`BUILD-02`/`MEM-02` + `PROV-03`/`SESS-03`/`TOOL-08`/`TOOL-09` (dead-code allows & speculative ports), `ERR-01`/`ERR-03`/`ERR-04` (missing justification comments), and `DUP-12`/`PROV-09` (function-kind constant).

## Index

Per-module findings:

- [agent module](modules/agent.md) — 8 findings
- [provider module](modules/provider.md) — 10 findings
- [tools module](modules/tools.md) — 12 findings
- [tui core (domain + application)](modules/tui-core.md) — 12 findings
- [tui infrastructure](modules/tui-infra.md) — 13 findings
- [memory module](modules/memory.md) — 14 findings
- [session module](modules/session.md) — 10 findings
- [sync module](modules/sync.md) — 11 findings
- [shared kernel + infra](modules/shared.md) — 15 findings
- [composition root + entrypoints](modules/root.md) — 10 findings

Cross-cutting findings:

- [architecture & layer invariants](cross-cutting/architecture-invariants.md) — 6 findings
- [cross-module duplication](cross-cutting/duplication.md) — 13 findings
- [security sweep](cross-cutting/security.md) — 7 findings
- [error-handling contract compliance](cross-cutting/error-handling.md) — 4 findings
- [file size & folder organization plan](cross-cutting/structure-modularization.md) — 14 findings
- [naming & convention consistency](cross-cutting/consistency.md) — 9 findings
- [build hygiene: clippy, dead code, dependencies](cross-cutting/build-hygiene.md) — 7 findings

---

*This document is the **scan deliverable** of the audit. It records what the read-only sweep found; it changes no source. Remediation — implementing the fixes, wave by wave, with tests and the definition-of-done gate — is a separate phase tracked from this index.*
