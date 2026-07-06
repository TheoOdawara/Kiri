# ADR 0018 — Native cross-platform support: portable home resolution + native file-tool I/O

- Status: Accepted
- Date: 2026-07-02

## Context

Kiri's "run on macOS, Linux, and Windows, all native" goal exposed two gaps, found by actually
running the tool-call surface on Windows rather than only compiling it (CI runs only `ubuntu-latest`
and `macos-latest`; no Windows runner exists yet).

**Home resolution was Unix-only.** `shared/infra/config/resolve.rs::home_dir()` and
`tools/application/path.rs::home()` each read only `$HOME`. Native `cmd`/PowerShell never set `HOME`
(confirmed on this machine: `$env:HOME` is empty, `$env:USERPROFILE` holds the real profile path), so
`~/.kiri` never expanded — `expand_tilde`/`expand_home_with` leave an unexpandable `~` verbatim,
landing config, credentials, memory, and sessions inside a literal `.\~\.kiri` under the current
working directory instead of the user's profile. Verified end-to-end: running the built `kiri.exe`
from a native PowerShell session (no `HOME` set) before this fix would have created that literal `~`
dir; after the fix it correctly writes `%USERPROFILE%\.kiri\config.toml`.

Three more Windows-only bugs surfaced from the same root cause — code that treated `Path::is_absolute()`
as sufficient to detect an absolute path, which is false for a Unix-style `/foo` on Windows (it needs a
drive prefix): `tools/infrastructure/sandbox.rs`'s `resolve_existing`/`resolve_create`/`relocated`,
`sync/application/sync_service.rs`'s local-path arm of `validate_remote_url`, and a `\`-vs-`/` display
bug in `memory/infrastructure/docs_library.rs`'s search-hit paths. All four are fixed alongside home
resolution; `tools/application/path.rs::is_absolute_target` already carried the right pattern
(`raw.starts_with('/') || expanded.is_absolute()`) for tool paths — it just hadn't propagated to the
sandbox's own resolution methods or to sync's URL validator.

**File-tool I/O was Unix-shell, Windows-native (ADR 0008).** ADR 0008 deliberately shelled the file
tools out to coreutils (`head`/`tee`/`rm`/`mv`/`ls`/`mkdir`/`grep`) on Unix, keeping native `std::fs`
bodies under `#[cfg(windows)]` only because "Windows-via-shell is deferred." That asymmetry is now the
wrong way around for a genuinely native, three-OS story: it makes every file tool depend on coreutils
being on `PATH` and GNU-vs-BSD flag compatibility, and it left the Windows-only helpers
(`support::read_capped`/`search_file`) and the Unix-only `run_fs_argv`/`exec::run_argv` machinery each
half-dead depending which platform compiled — exactly the condition that made
`cargo clippy --all-targets -D warnings` (the project's own lint gate) fail on Windows for
`exec_cwd_for`, `is_outside_root`, `stderr_text`, `message`, and `run_argv`, all unreachable once their
only callers (the Unix argv arms) don't compile. This was invisible because CI never runs Windows.

## Decision

**Home resolution: one cross-platform source.** New `shared/infra/home::home_dir() -> Option<PathBuf>`:
`$HOME` first, then `%USERPROFILE%`, then the pre-Vista `%HOMEDRIVE%%HOMEPATH%` pair. Both prior
`$HOME`-only readers now delegate to it, so config and agent tool-path tilde expansion can never
disagree on the user's home. The env path-list separator (`KIRI_SANDBOX_RO_PATHS`/`RW_PATHS`) is now
`:` on Unix / `;` on Windows via a `#[cfg]` split, matching each platform's own `PATH` convention.

**Absolute-path detection: one predicate, reused.** `tools/application/path::is_absolute_path(raw,
expanded)` (`raw.starts_with('/') || expanded.is_absolute()`) is now the single source `is_absolute_target`
wraps and `sandbox.rs`'s three resolution methods call directly; `sync_service.rs`'s local-path arm gets
the equivalent inline check (kept local — it's a different bounded context, and one more call site of a
two-line idiom does not yet justify a shared cross-module abstraction).

**Windows credential/config writes gain crash-atomicity.** The `#[cfg(not(unix))]` fallback in
`provider/infrastructure/secrets/file_store.rs` and `sync/infrastructure/memory_ndjson.rs` routed
through a bare `fs::write`; both now route through the already-portable `write_atomic`/`write_atomic_sync`
(temp sibling + rename) in `shared/infra/fs.rs`. Owner-only *permission* (0600/0700 on Unix) still has
no Windows equivalent without ACL APIs — accepted as-is; the user-profile DACL inheritance remains the
documented fallback, tracked toward the OS-confinement work (a future ADR) rather than solved here.

**File-tool I/O: native `std::fs` everywhere, ADR 0008 superseded.** All eight file tools
(`read_file`/`write_file`/`edit_file`/`delete_file`/`move_path`/`list_dir`/`create_dir`/`delete_dir`)
now share one native body per tool on every platform — the former `#[cfg(windows)]` arms, promoted.
`edit_file` was already there (its `python3` shell-out was removed earlier because a clean macOS has no
`python3` on `PATH` — the same portability argument, generalized). `support.rs`'s `read_capped`/
`search_file` are unconditional; `search`'s recursive walk (symlink-refusing, `SECRET_DIRS`-pruning,
sensitive-name-filtering) is the sole implementation. `move_path` gains a same-process cross-device
fallback (`rename_or_copy`): `std::fs::rename` first, and only on `ErrorKind::CrossesDevices` **and**
when the source is a plain file, copy-then-remove — `mv`'s old cross-filesystem behavior, restored
without a shell. A cross-device **directory** move is not given this fallback: safely recursing into a
tree while never following a nested symlink into an arbitrary target is a feature of its own, not a
one-line addition to this migration; it surfaces the plain `CrossesDevices` error instead of guessing.

**Now-dead machinery retired, not `#[allow(dead_code)]`-suppressed:** `tools/infrastructure/support.rs`'s
`run_fs_argv`, `exec::run_argv`, `Sandbox::exec_cwd_for`/`is_outside_root`, `ExecResult::stderr_text`/
`succeeded`, and `ExecError::message` are deleted outright — each had zero callers left once the Unix
argv arms were gone (confirmed by grep before deletion). `exec::run`'s concurrent stdin-writer/drainer
is removed too: it existed solely to feed `tee`'s stdin, and no caller feeds stdin to a spawned process
anymore (`run_shell`, the sole remaining caller, always passes none). `exec.rs` is now exclusively the
`run_command` shell-out path (`sh -c` / `cmd /C`) — file tools never reach it.

**Preserved exactly:** every tool's model-facing ID, JSON schema, pt-BR confirmation prose, and
`command_line` label (the `characterization.rs` snapshot test still passes unchanged) — only the I/O
mechanism moved. The same caps (64 KiB read, 100 matches, 200 chars/line) and the same secret-dir/
sensitive-name pruning apply, now in one Rust implementation instead of duplicated across a grep-flag
version and a native version.

## Consequences

- New `shared/infra/home.rs` (pure, unit-tested fallback chain). Two call sites collapse to it.
- `tools/infrastructure/sandbox.rs`, `sync_service.rs`, `docs_library.rs` each get a small, targeted fix
  for the `/`-vs-drive-letter absolute-path gap; regression tests for all three now pass natively on
  Windows (previously three baseline test failures, confirmed pre-existing via `git stash` before this
  work started, invisible because CI never ran Windows).
- `run_command` is now the **only** shell-out surface in the tools layer. This sharpens (does not widen)
  the existing security boundary: ADR 0009's OS-level confinement (Seatbelt today, Linux/Windows
  adapters tracked as debt) already targeted `run_command` as the high-risk arbitrary-shell surface; the
  file tools' real guard was always the `Sandbox` path-validation chokepoint (canonicalize, `..`
  rejection, sensitive-name and secret-dir refusal), which is untouched and 100% `std::fs`-based
  already. **Trade-off, stated plainly:** on macOS, the file tools used to route through the Seatbelt
  confiner (getting its network-deny/write-confinement as defense-in-depth); native `std::fs` calls
  don't pass through the confiner (there is nothing to confine — no child process, no argv). ADR 0008
  already argued the file tools are the strong case for this (one validated absolute path, no shell); this
  removes a redundant-but-real backstop layer for them specifically. Accepted: the residual risk is a
  validated, canonicalized path performing an operation no different from what the Seatbelt profile's
  `(allow default)` base already permitted for the workspace root and configured extras.
- `cargo clippy --all-targets -- -D warnings` — the project's documented lint gate — now passes clean on
  Windows for the first time; previously it failed with five `dead_code` errors purely because the
  Unix-only production callers and the Unix-gated exec.rs test module both vanished on a Windows compile,
  leaving genuinely-Unix-only code unreachable in that build. No CI runner caught this (tracked: add
  `windows-latest` to `.github/workflows/ci.yml`, not done in this change).
- Coverage added, not just moved: `write_file`/`delete_file`/`create_dir`/`delete_dir`/`move_path`/
  `read_file`/`list_dir` previously had **zero** `execute()`-level tests (only schema/confirmation-prompt
  checks existed, in `characterization.rs`); each now has real-filesystem regression tests (happy path +
  at least one refusal/error path) exercised against a `tempfile::TempDir`, closing a pre-existing gap
  this migration's own correctness depended on catching.
- **Deferred, tracked as follow-up (a future ADR):** the OS-level command-sandbox jail (ADR 0009) still
  exists only on macOS; Linux and Windows `run_command` remain confirmation-only. This ADR does not
  change that — it only removes the coreutil/GNU-vs-BSD fragility and the home-resolution bug that block
  Kiri from running correctly on Windows at all, which had to land first.
- **Follow-up closed:** promoting the `#[cfg(windows)]` bodies to the sole cross-platform implementation
  carried over a gap tracked as issue #9 — those bodies did direct sync `std::fs` I/O on the async
  runtime thread with no timeout, a defect that (by this ADR's own change) stopped being Windows-only
  and became cross-platform the moment the Unix-shell arm was deleted. All nine file tools
  (`read_file`/`write_file`/`edit_file`/`create_dir`/`delete_file`/`delete_dir`/`move_path`/`list_dir`/
  `search`, plus the shared `support::read_capped`/`search_file`/`ensure_parent_dirs`) now route their
  I/O through `tokio::fs` bounded by `tokio::time::timeout(exec::DEFAULT_TIMEOUT, …)`, mirroring
  `edit_file`'s pre-existing pattern — the one file tool that was never affected by this gap.
