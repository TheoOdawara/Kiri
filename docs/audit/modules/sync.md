# Audit — sync module

> Scope: `src/modules/sync.rs`, `src/modules/sync/application/{mod.rs,git.rs,sync_service.rs}`, `src/modules/sync/domain/{mod.rs,merge.rs}`, `src/modules/sync/infrastructure/{mod.rs,git_cli.rs,memory_ndjson.rs}` (read in full); cross-checked against `src/main.rs`, `src/modules/tui/infrastructure/runtime.rs`, `src/shared/infra/config.rs`, `src/shared/kernel/provider.rs`, and `src/modules/memory/infrastructure/file_project_memory.rs`.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The sync module is in good overall health: the `Git` port is clean and dyn-friendly, the git adapter has a real timeout with kill-on-drop, secrets are kept out of the work-tree by both `.gitignore` and a deliberate copy-allowlist, and the config trust gate is well-commented with a strong test suite. The notable issues are concentrated in the security trust gate (`risky_config_changes`) and the internal layering of `SyncService`: the gate has a real coverage hole (a `sandbox.mode = require → os` downgrade is not flagged), it is keyed to hand-typed string literals decoupled from the real `AuthMethod`/sandbox enums, and `import` reads an untrusted remote file fully into memory before its entry cap applies. Architecturally, pure trust-policy logic lives in the application file (inconsistent with `merge.rs` in `domain/`), the application service performs filesystem I/O inline instead of behind an adapter like `memory_ndjson`, and `write_atomic` is duplicated with the memory module.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 5 | 5 |

## Findings

### [SYNC-01] Trust gate misses the `sandbox.mode = require → os` downgrade
- **Severity:** High
- **Category:** security
- **Location:** `src/modules/sync/application/sync_service.rs:349-353`, cross-ref `src/shared/infra/config.rs:471-479`
- **Problem:** `risky_config_changes` only flags the sandbox when the incoming mode is exactly `"off"`. But `resolve_sandbox_mode` recognizes three modes — `off`, `require` (refuse `run_command` when no OS sandbox is available), and the default `os`. A synced config that drops a hardened `mode = "require"` down to `os` (or omits `[sandbox]` entirely) is a genuine weakening of the sandbox posture (`run_command` will now run unconfined on a platform with no OS sandbox), yet it produces an empty risk list and is applied without `--force`. The gate's stated purpose is "weaken the sandbox" defense, so this is a hole in the security control itself.
- **Evidence:**
```rust
if incoming.sandbox.mode.as_deref() == Some("off")
    && current.sandbox.mode.as_deref() != Some("off")
{
    risks.push("sandbox mode set to 'off'".to_string());
}
```
- **Recommendation:** Treat any relaxation of the mode as risky, not just `→ off`. Rank the modes (`require` strictest, then `os`, then `off`) and flag when the incoming rank is strictly weaker than current (covering `require → os`, `require → off`, and `os → off`). Add a regression test for the `require → os` case.

### [SYNC-02] Trust gate keyed to hand-typed magic strings decoupled from the real enums
- **Severity:** Medium
- **Category:** magic
- **Location:** `src/modules/sync/application/sync_service.rs:323`, `:349`, `:354-360`; cross-ref `src/shared/kernel/provider.rs:83-86`
- **Problem:** The security-critical gate compares against bare literals `"none"`, `"off"`, `"allow"`, `"deny"` that mirror the serialized form of `AuthMethod` and the sandbox/network options, but are typed by hand here rather than derived from those types. `AuthMethod`'s wire strings live in `provider.rs` (`api-key`/`none`/`oauth`); if any of those serialized spellings ever change, this gate silently stops detecting downgrades — a security regression with no compile error and no test failure (the tests also hard-code the same strings). `TrustView` also re-declares a parallel `Option<String>` shadow of the config schema instead of reusing the typed `AuthMethod`.
- **Evidence:**
```rust
if cur.auth.as_deref() != Some("none") && inc.auth.as_deref() == Some("none") {
    risks.push(format!("provider '{id}' auth disabled (set to none)"));
}
```
- **Recommendation:** Parse `auth` into the real `AuthMethod` (and the sandbox mode/network into the same types `config.rs` uses) so the gate reasons over the same definitions as the loader, or expose named constants from the enum modules and reference them here. At minimum, add a test asserting the literals equal `AuthMethod::None.as_str()` etc., so a rename breaks the build.

### [SYNC-03] `import` reads the whole untrusted NDJSON into memory before the entry cap applies
- **Severity:** Medium
- **Category:** security
- **Location:** `src/modules/sync/infrastructure/memory_ndjson.rs:54`, `:13-17`, `:64-66`
- **Problem:** `IMPORT_CAP` is documented as the guard against a "large or hostile remote `memory.ndjson`", but it only bounds the number of *entries processed* inside the loop. The file is first slurped whole with `fs::read_to_string(path)`, so a multi-gigabyte (or adversarially crafted) remote file causes an unbounded heap allocation before the cap is ever consulted — a memory-exhaustion DoS that the cap does not actually prevent. `pull` does `git reset --hard FETCH_HEAD` first, so whatever the remote ships lands on disk and is then read in full.
- **Evidence:**
```rust
let content = fs::read_to_string(path).await?;
// ...
for line in content.lines() {
    // ...
    if report.merged + report.skipped >= IMPORT_CAP { break; }
```
- **Recommendation:** Stream the file line-by-line with a buffered reader (e.g. `tokio::io::AsyncBufReadExt::lines`) so memory stays bounded, and/or reject the file when its size exceeds a documented byte cap before reading. Align the comment with the actual guarantee.

### [SYNC-04] Pure config-trust policy lives in the application file instead of `domain/`
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/sync/application/sync_service.rs:250-366` (`TrustView`/`TrustProvider`/`TrustSandbox`/`TrustEmbeddings` + `risky_config_changes`); contrast `src/modules/sync/domain/merge.rs:11`
- **Problem:** `risky_config_changes` and its `TrustView` family are pure data/rules with no I/O — the textbook definition of `domain/` in this codebase. The sibling pure rule `incoming_wins` was correctly placed in `domain/merge.rs`, so the placement here is inconsistent and also burdens `sync_service.rs` with two responsibilities (git/fs orchestration *and* the security trust policy). Co-locating the gate with the I/O makes it easy to overlook when reasoning about the security surface.
- **Evidence:**
```rust
// sync_service.rs (application layer) — pure, no I/O, yet not in domain/
fn risky_config_changes(current: &str, incoming: &str) -> Vec<String> { /* ... */ }
```
- **Recommendation:** Extract the `TrustView` structs + `risky_config_changes` into `src/modules/sync/domain/config_trust.rs` (mirroring `merge.rs`), keeping the unit tests with the logic. `sync_service.rs` then just calls it.

### [SYNC-05] `write_atomic` duplicated between sync and memory
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/sync/application/sync_service.rs:239-248`, `src/modules/memory/infrastructure/file_project_memory.rs:21-26`
- **Problem:** Two private `write_atomic` helpers implement the identical write-temp-then-rename pattern (only the temp-name strategy differs: `.{name}.kiri-tmp` vs `path.with_extension("json.tmp")`). This is a single durability primitive copied across modules; a fix or hardening to one (e.g. fsync before rename) will not reach the other.
- **Evidence:**
```rust
// sync_service.rs
let tmp = path.with_file_name(format!(".{name}.kiri-tmp"));
fs::write(&tmp, contents).await?;
fs::rename(&tmp, path).await?;
```
- **Recommendation:** Promote a single `write_atomic` to a shared infra util (e.g. `src/shared/infra/`) and have both call sites use it. Pick one temp-name convention.

### [SYNC-06] `SyncService` (application) performs filesystem I/O inline instead of behind an adapter
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/sync/application/sync_service.rs:61`, `:68`, `:127`, `:136`, `:149`, `:200-204`, `:245-246`
- **Problem:** Git access is correctly abstracted behind the `Git` port, and the NDJSON read/write is correctly encapsulated in the `memory_ndjson` infrastructure adapter — yet the same service reaches straight for `tokio::fs::{create_dir_all,write,read_to_string,copy,rename}` for the work-tree, `.gitignore`, and config copy. The result is an inconsistent layer split inside one module: half the I/O is behind an adapter, half is inline in the use-case. (Note: CLAUDE.md permits the sync *context* to own its data dir, so this is a layering inconsistency, not a breach of the network/fs invariant — but it diverges from the module's own `memory_ndjson` pattern.)
- **Evidence:**
```rust
async fn export_profile(&self, dir: &Path) -> Result<usize> {
    fs::write(dir.join(".gitignore"), GITIGNORE).await?;
    if self.config_path.exists() {
        fs::copy(&self.config_path, dir.join(CONFIG_FILE)).await?;
    }
    memory_ndjson::export(self.memory, &dir.join(MEMORY_FILE)).await
}
```
- **Recommendation:** Move the work-tree filesystem operations (dir/gitignore/config materialization + atomic config write) into an infrastructure adapter behind a small port, consistent with `memory_ndjson`, so the application service orchestrates only ports.

### [SYNC-07] Bare `let _ =` on `remote remove` without a why-it-is-safe justification
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/sync/application/sync_service.rs:69-70`
- **Problem:** The error contract requires a deliberately-ignored fallible call to carry a one-line justification of *why dropping the error is safe*. The comment here explains the *intent* ("replacing any existing one so re-init can repoint it") but not why the error is safe to discard — namely that `remote remove origin` fails harmlessly when no `origin` exists yet, which is the normal first-init case. Contrast line 65-66, which justifies its ignore correctly ("Best-effort: renaming an unborn branch can no-op").
- **Evidence:**
```rust
// Set the remote, replacing any existing one so re-init can repoint it.
let _ = self.git.run(&["remote", "remove", "origin"], &dir).await;
```
- **Recommendation:** Extend the comment to state that a missing `origin` is expected and the failure is intentionally ignored (or assert success only when a remote actually existed).

### [SYNC-08] `incoming_wins` parse-failure fallback re-introduces the exact bug it warns against
- **Severity:** Low
- **Category:** error-handling
- **Location:** `src/modules/sync/domain/merge.rs:11-19`
- **Problem:** When either timestamp fails to parse as RFC3339, the function falls back to `incoming_updated_at > existing_updated_at` — the lexicographic string compare the doc-comment explicitly calls "WRONG". A malformed `updated_at` in a synced entry (e.g. `"zzzz"`) would then string-compare as greater than any real RFC3339 instant and overwrite the local entry. Since `MemoryEntry.updated_at` is a free `String` (no validation), a crafted/corrupt NDJSON line can steer the merge through this path.
- **Evidence:**
```rust
(Ok(incoming), Ok(existing)) => incoming > existing,
_ => incoming_updated_at > existing_updated_at,
```
- **Recommendation:** On parse failure of either side, default to keeping the existing entry (return `false`) — fail-closed for a non-comparable timestamp — rather than reusing the known-wrong compare. Add a test for a non-RFC3339 incoming timestamp.

### [SYNC-09] User-supplied remote URL passed to git without a `--` argument terminator
- **Severity:** Low
- **Category:** security
- **Location:** `src/modules/sync/application/sync_service.rs:71-72`
- **Problem:** `git remote add origin <remote_url>` forwards the URL positionally with no `--` separator. A URL beginning with `-` would be parsed by git as an option rather than the remote URL. The URL is user-controlled (`kiri sync init <url>`), so this is defense-in-depth rather than an external-input injection, and `Command::args` already prevents shell injection (no shell is invoked) — but a leading-dash URL is a confusing failure mode.
- **Evidence:**
```rust
self.git_ok(&["remote", "add", "origin", remote_url], &dir)
    .await?;
```
- **Recommendation:** Insert `--` before the URL (`["remote", "add", "origin", "--", remote_url]`) or validate the URL scheme up front, so a value starting with `-` cannot be interpreted as a flag.

### [SYNC-10] `pull` parses the incoming config redundantly; one branch of the gate is dead on that path
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/modules/sync/application/sync_service.rs:130` and `:292-306`
- **Problem:** In `pull`, the incoming config is parsed by `config::validate_config_str` (full `RawConfig`) and, immediately after, by `risky_config_changes` as `TrustView` — two parses of the same string per pull, plus the current config parsed again. Because the caller validates schema first, the "incoming config is not valid TOML" arm inside `risky_config_changes` (lines 293-295) is unreachable in the real flow and only exercised by direct unit tests, making it effectively dead on the production path.
- **Evidence:**
```rust
let incoming: TrustView = match toml::from_str(incoming) {
    Ok(value) => value,
    Err(error) => return vec![format!("incoming config is not valid TOML: {error}")],
};
```
- **Recommendation:** Either have `validate_config_str` return the parsed `RawConfig` so the gate reuses it, or document that `risky_config_changes` is also a standalone entry point and keep the guard intentionally. Minor; flagged for clarity, not correctness.

### [SYNC-11] Exported memory NDJSON written with default (world-readable) permissions
- **Severity:** Low
- **Category:** security
- **Location:** `src/modules/sync/infrastructure/memory_ndjson.rs:40`, cross-ref `src/modules/sync/application/sync_service.rs:200`
- **Problem:** `memory.ndjson` holds the user's cross-project memory (facts and `preference` entries — potentially personal/sensitive content) and is written via `fs::write` with default `0644` permissions into `~/.kiri/sync`. Unlike the credentials file (kept `0600` by design), this export is readable by other local users on a shared host. It is non-secret by the project's threat model, but it is more sensitive than a build artifact.
- **Evidence:**
```rust
fs::write(path, body).await?;
Ok(entries.len())
```
- **Recommendation:** Create the sync work-tree files with `0600` (or restrict the `~/.kiri/sync` directory mode) on Unix, consistent with the credentials-file handling, so synced personal memory is not exposed on multi-user machines.

## Strengths
- The `Git` port (`application/git.rs`) is a clean, capability-named, dyn-compatible trait with an explicit contract (non-zero exit reported via `GitOutput.success`, not `Err`), and the `GitCli` adapter has a real `GIT_TIMEOUT` with `kill_on_drop(true)` and `stdin(Stdio::null())` — exactly the I/O-timeout discipline the contract demands.
- Secret hygiene is layered and tested: the `.gitignore` excludes `credentials.json`/`*.db`/`embeddings.json`, `export_profile` copies only the non-secret config, and a test explicitly asserts `credentials.json` never lands in the work-tree.
- `merge::incoming_wins` correctly parses RFC3339 instants instead of comparing variable-width timestamp strings, with a precise doc-comment and targeted tests for the fractional-second cases — a subtle bug avoided deliberately.
