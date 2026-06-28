# Audit — Error-handling contract compliance (cross-cutting)

> Scope: the whole `src/` tree, read in full, with `#[cfg(test)]`/`#[test]` code excluded from the panic rules. Deep focus on every `let _ =`, `.ok()`, `.unwrap_or_default()`, `.unwrap_or(`, `unwrap()`, `expect(`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, plus I/O-timeout coverage, per-turn lifecycle-flag resets, and silent-no-op-on-user-intent.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary

The error-handling contract is met to an unusually high standard. There are **zero** runtime-reachable `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`/`unimplemented!` in production code (every hit is inside `#[cfg(test)]`, verified with a per-file test-boundary scan). **Every** I/O surface is timeout-bounded: provider HTTP (`connect_timeout` + streaming-safe `read_timeout`), process exec (`tokio::time::timeout` + `kill_on_drop`), the `git` subprocess (120s + `kill_on_drop`), SQLite (a 5s `DB_OP_TIMEOUT` over `spawn_blocking`), and the embeddings call (a 5s `EMBED_TIMEOUT`). The streaming accumulators additionally bound untrusted provider output by byte count (`MAX_STREAM_BYTES`). The per-turn `busy` flag is reset on every exit path (`Msg::TurnEnded` after `on_turn_end`, plus the distillation's own reset), including the draw-failure path, which deliberately `break`s rather than `?`-propagates so cleanup always runs — and there is a regression test (`provider_failure_propagates_after_finishing_the_render`) locking that behavior. The overwhelming majority of `let _ =` / `.ok()` / `.unwrap_or_*` sites carry the required one-line justification.

The remaining findings are all **Low**: two `let _ = draw_and_copy(...)` sites missing the justifying comment the contract requires for a bare ignored `Result`; one credential-persist failure that is silently swallowed on the live-swap path while the equivalent startup path surfaces it; an undocumented `canonicalize()` degradation repeated in three places; and one `let _ = <non-fallible>` that reads like a swallowed `Result` but is an unused-parameter suppression. None affect correctness, security, or data integrity.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 0 | 0 | 4 |

## Findings

### [ERR-01] Two bare `let _ = draw_and_copy(...)` lack the required justification comment
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/tui/infrastructure/runtime.rs:1365`, `src/modules/tui/infrastructure/runtime.rs:1473`
- **Problem:** The contract states a bare `let _ = <fallible>` is a defect unless it carries a one-line justification. `draw_and_copy` returns `io::Result<()>`. Every *other* ignored draw Result in this file is either explicitly handled (the turn loop `break`s with `AgentError::Io` at line 934, the distillation loop `break`s `None` at line 1495) or commented. These two pre-operation progress draws — the "sincronizando (push)…" draw before the network push, and the "destilando…" draw before the distillation loop — drop the Result with no inline rationale. A failed progress draw here is benign (the operation proceeds), but the missing one-liner is the exact pattern the contract flags, and it makes the two reads indistinguishable from an accidental swallow.
- **Evidence:**
```rust
// sync_push (~1364)
model.render_at = Some(Instant::now());
let _ = draw_and_copy(terminal, model);   // no "why ignored" comment

// drive_distillation (~1472)
model.render_at = Some(started);
let _ = draw_and_copy(terminal, model);   // no "why ignored" comment
```
- **Recommendation:** Add the same kind of one-line justification used elsewhere (e.g. `// Best-effort progress paint; a failed pre-op draw must not block the sync/distillation that follows.`). Do not change behavior.

### [ERR-02] Live-swap credential-persist failure is swallowed where the startup path surfaces it
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/tui/infrastructure/runtime.rs:160` vs `src/modules/agent/../app.rs:411-420` (`src/app.rs:411`)
- **Problem:** Both `ProviderSwap::resolve_credential` (runtime) and `resolve_credential` (composition root) import an API key from an env var and try to persist it to the secret store. The startup path reports a persist failure (`eprintln!` "could not persist the credential … using it this session only"); the live-swap path drops it with `let _ = self.secrets.set(...)` and only a "non-fatal" comment. The swallow is contract-compliant (it carries a justification), but the divergence means a user who switches live to an env-keyed provider is never told the key was not saved, so the next session silently needs the env var again. This is a consistency gap, not a correctness bug.
- **Evidence:**
```rust
// runtime.rs:158-161 — silent on failure
// Best-effort persist so a later switch needs no env var; a store failure is non-fatal —
// the credential still works for this swap.
let _ = self.secrets.set(&profile.id, &credential);
```
- **Recommendation:** Make the two paths consistent — either surface a transcript `Notice` on the live-swap persist failure (the runtime already has the model in hand), or document explicitly why the live path is intentionally quieter than startup.

### [ERR-03] Undocumented `canonicalize()` degradation repeated in three places
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/app.rs:67`, `src/app.rs:186`, `src/modules/tui/infrastructure/runtime.rs:584`
- **Problem:** `path.canonicalize().unwrap_or_else(|_| path.clone())` (and the `to_path_buf()` variant) silently falls back to the literal path when canonicalization fails. The fallback is safe and deliberate, but the codebase's otherwise-uniform discipline is to comment every `.unwrap_or_*`/degradation with a one-line "why"; these three are the only `.unwrap_or_*` sites that degrade an I/O result without that note, so they read as unexplained against the surrounding style.
- **Evidence:**
```rust
// app.rs:65-69
let canonical_path = settings
    .path
    .canonicalize()
    .unwrap_or_else(|_| settings.path.clone());
```
- **Recommendation:** Add a shared one-liner at each site (e.g. `// A non-canonicalizable path (missing/permission) degrades to the literal path; project_id keying still works.`). Optionally factor the three identical fallbacks into one helper to remove the duplication too.

### [ERR-04] `let _ = blocks;` reads like a swallowed Result but is an unused-parameter suppression
- **Severity:** Low
- **Category:** naming
- **Location:** `src/modules/tui/infrastructure/markdown.rs:278`
- **Problem:** `start(&mut self, tag: Tag, blocks: &mut Vec<Block>)` never uses `blocks` (only the sibling `end` does), and the unused parameter is silenced with `let _ = blocks;`. It is *not* a fallible value, so it is not a swallowed-failure defect — but it is the single `let _ =` in the tree that an auditor hunting for dropped `Result`s must stop and disprove, which is precisely the cost the contract's "no bare `let _ =`" rule exists to avoid.
- **Evidence:**
```rust
Tag::Link { .. } | Tag::Image { .. } | Tag::FootnoteDefinition(_) => {}
_ => {}
}
let _ = blocks;   // suppresses unused-param warning on an &mut Vec<Block>
```
- **Recommendation:** Rename the parameter `_blocks` (idiomatic, self-documenting) or drop it from `start`'s signature if symmetry with `end` is not required. No behavior change.

## Strengths

- **Panic-free runtime paths.** A per-file test-boundary scan confirms zero `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`/`unimplemented!` reachable outside `#[cfg(test)]`. Even genuinely-total helpers stay total (`now_rfc3339` uses `.unwrap_or_default()` with a comment; `present_plan::execute` echoes the plan instead of asserting unreachability).
- **Exhaustive, uniform timeout coverage with regression tests.** Provider HTTP, exec (`kill_on_drop`), git, SQLite (`run_blocking` + timeout), and embeddings are all bounded, and the provider adapters carry hermetic "accepts-but-never-responds → fails fast" tests (`complete_fails_fast_when_the_provider_accepts_but_never_responds`) that lock the original hang regression. Untrusted stream size is independently capped by `MAX_STREAM_BYTES`, with the reasoning written out at `openai/sse.rs:13`.
- **Disciplined swallow-justification and lifecycle resets.** Nearly every ignored `Result`/`Option` carries a one-line "why it is safe", failures reach the user as transcript `Notice`s rather than silent no-ops (the onboarding submit gate, `apply_set_effort`/`apply_set_provider`, `flush_session`, `on_turn_end`'s empty-completion notice), and `busy` is reset on every exit path including the draw-failure path — with `provider_failure_propagates_after_finishing_the_render` proving cleanup runs before an error propagates.
