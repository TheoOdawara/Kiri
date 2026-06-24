# ADR 0008 — Shell-backed file tools (same surface, terminal-command I/O)

- Status: Accepted
- Date: 2026-06-24

## Context

The file tools (`read_file`, `write_file`, `edit_file`, `delete_file`, `move_path`, `list_dir`,
`create_dir`, `delete_dir`, `search`) executed their I/O with native Rust `std::fs`; only
`run_command` shelled out. We want the agent to operate the workspace through **terminal commands**
instead of bespoke Rust primitives — the model keeps calling the same tool IDs with the same schemas,
but each tool's execution is the equivalent terminal command. We also want the workspace to be a hard
**jail**: commands run confined to the repo, and leaving it is an explicit, user-approved excursion
after which the harness returns.

This reinterprets — but does not remove — the "single path chokepoint" invariant of ADR 0002.

## Decision

**The model-facing surface is unchanged.** Same 10 tool IDs, same JSON schemas, same pt-BR
confirmation prose and `command_line` labels. The characterization snapshot
(`snapshots/characterization.json`) stays byte-identical. Only each `Tool::execute` body changes.

**Layer 0 (path validation) stays in Rust, untouched.** Every tool still resolves its path through the
`Sandbox` — lexical `..` rejection, `canonicalize`, `starts_with(root)`, and the sensitive-file guard
(ADR 0002 + the sensitive-file guard). The sandbox returns a validated **absolute** path; that path is
the only thing handed to the command. The chokepoint moves nowhere — what changes is the I/O
mechanism after validation, from `std::fs` to a terminal command.

**Execution goes through one helper** (`tools/infrastructure/exec.rs`):

- `run_argv(argv, cwd, stdin, env, timeout)` — builds `Command::new(prog).args(...)` directly, **no
  `sh -c`**. The validated absolute path is its own OS-level argument, so there is no shell quoting,
  word-splitting, or injection. `stdin` feeds raw bytes (e.g. `write_file`'s content); `env` passes
  values without interpolating them into the command (e.g. `edit_file`'s old/new strings). Stdin is
  `/dev/null` when no input is supplied, so a command that would otherwise prompt (`rm` on a
  write-protected file) sees EOF instead of hanging.
- `run_shell(script, cwd, timeout)` — `sh -c` / `cmd /C`, used only by `run_command`.
- Shared plumbing: piped stdio, a stdin writer that runs concurrently with the stdout/stderr drain (so
  a child echoing its input like `tee` cannot deadlock against a full pipe), `kill_on_drop`, a timeout,
  and the 64 KiB output cap.

**Per-tool translation (Unix):** `read_file`→`head -c (cap+1)`, `write_file`→`tee` (content via stdin),
`delete_file`→`rm`, `move_path`→`mv`, `list_dir`→`ls -1A -p` (re-sorted in Rust),
`create_dir`→`mkdir -p`, `delete_dir`→`rm -rf`, `search`→`grep -rIFn`. Pre-flight guards that are not
path validation (refuse the root, refuse the wrong file type, create missing parent dirs, the size cap,
the empty-string checks) and each tool's fixed result string stay in Rust.

**`edit_file` uses `python3`** — exact, literal, first-occurrence multiline replacement has no faithful
Unix coreutil (`sed` is line/regex-oriented). A `python3 -c` one-liner reads the file, `find`s the first
literal occurrence of `$KIRI_OLD`, splices in `$KIRI_NEW`, and writes back; the strings travel via the
environment, never the command line; exit code 3 signals "not found". The allowed execution surface is
therefore **coreutils + `python3`** — no `perl`, and `search` uses `grep` (not `rg`, which may be
absent).

**The jail and leaving it.** The workspace root is the jail; commands run with `cwd` at the root.
Leaving the jail is gated by the existing location-based confirmation (ADR 0002, 2026-06-21): an
absolute/`~` target defaults to **decline** `[s/N]` — that *is* the permission request. On approval, the
command runs with `cwd` at the target's nearest existing directory (`Sandbox::exec_cwd_for`) — the
harness "moves" there for that one call and, because each call builds its own process, is back at the
root for the next. No process-global `chdir`.

**Unix-first.** The shelled bodies are `#[cfg(unix)]`; the native `std::fs` bodies are kept under
`#[cfg(windows)]`, so no platform regresses and no Windows test breaks. Windows-via-shell is deferred.

## Consequences

- New `tools/infrastructure/exec.rs`; `run_command` is reduced to use it. The sandbox gains
  `is_outside_root` / `exec_cwd_for` (purely additive). `support.rs`'s `read_capped` / `search_file`
  (and their constants) are now Windows-only (`#[cfg(windows)]`); the Unix paths use `head`/`grep`.
- The file tools now require their utilities (`head`, `tee`, `rm`, `mv`, `ls`, `mkdir`, `grep`) and
  `python3` on `PATH`. The per-tool tests become integration-style on Unix.
- **Fidelity deltas, accepted and documented:** `search` drops the native 1 MiB per-file skip (grep
  scans large text files) and re-applies the 100-match / 200-char caps and the `path:line: ` format in
  Rust over grep's output; `list_dir` relies on `ls -p` for the directory marker (a symlink-to-directory
  may be marked where the native `DirEntry::file_type` did not) and re-sorts in Rust;
  `move_path` now succeeds across filesystems (`mv` copies where `fs::rename` returned `EXDEV`).
- **Security.** Because `run_argv` passes the validated absolute path as an argv element (no shell), the
  Layer 0 guarantee is preserved as strongly as before for the file tools — stronger than a free-form
  shell tool would be. `run_command` remains the unconstrained surface, already tracked under ADR 0002's
  OS-level-sandbox `security-debt`. `edit_file`'s old/new strings via environment (not argv/shell)
  keep arbitrary content uninterpreted.
