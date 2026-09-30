# ADR 0026 — Env scrubbing for shell-exec children (`run_command` + hooks)

- Status: Accepted
- Date: 2026-07-05
- Relates to: ADR 0009 (OS-level command sandbox), ADR 0021 (extensions framework — hooks)

## Context

Audited as issues #25 and #49. `tokio::process::Command` inherits the **entire** parent process
environment by default. `exec::run_shell` — the sole place `run_command` and hooks (`ShellHookRunner`,
routed through the same function) spawn a shell — never called `env_clear()`, so a model-supplied
`run_command` invocation, or a project-authored hook script, could read back any harness secret the parent
process holds via a plain `env`/`printenv`: provider API keys (`NVIDIA_API_KEY`, `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, any `KIRI_<ID>_API_KEY`), whatever else happens to be in the user's shell environment
when Kiri was launched.

This is exactly the gap MCP server children already close: `RmcpConnection::connect`
(`rmcp_client.rs`) calls `cmd.env_clear()` then re-applies a curated allowlist. `run_command`/hooks had no
equivalent.

## Decision

`exec::run_shell` now scrubs the child's environment the same way, via a new `scrub_env` helper:
`cmd.env_clear()` then re-apply only `INHERITED_ENV_VARS` — `PATH`, `HOME`, `USERPROFILE`, `SystemRoot`,
`APPDATA`, `LOCALAPPDATA`, `TEMP`, `TMP`, `SHELL`, `TERM`, `LANG`, `LC_ALL` — the vars a normal shell
script or dev/package tool (cargo, npm, git, …) needs to resolve and run, never a secret. The allowlist is
a separate local constant in `exec.rs`, not shared with MCP's: the two contexts scrub for different
process shapes (an interactive shell script vs. a headless server binary) and the plan explicitly calls
for replicating the pattern, not centralizing it — a shared constant would be one more cross-module
coupling for two lists that may legitimately diverge.

Both `run_command` and hooks route through this one function (`exec::run_shell`), so this single change
closes both #25 and #49 at once — the root-cause fix, not a guard duplicated at each caller.

### A composition bug this surfaced: OS-confinement adapters must also clear, not just replay

Fixing `run_shell` alone was not sufficient. On macOS/Linux, `run_shell` hands its (now-scrubbed) `Command`
to `confiner.confine()` (`MacosSeatbelt`/`BwrapSandbox`), which **rebuilds a fresh `Command`** wrapping the
original in `sandbox-exec`/`bwrap`, carrying over the inner command's env by iterating
`cmd.as_std().get_envs()`. `get_envs()` only reports **explicit overrides** made via `env`/`env_remove`
— it has no way to report that `env_clear()` was called on the original `Command`. Verified empirically
(a throwaway probe binary): a fresh `Command` that replays only the explicit overrides from a
`env_clear()`'d source, without calling `env_clear()` on itself, still inherits the **full** ambient
environment — the rebuild silently undid the scrub. Both `MacosSeatbelt::confine` and
`BwrapSandbox::confine` now call `wrapped.env_clear()` before replaying the collected overrides, closing
this for real on the platforms where OS confinement is actually active (macOS is the v1 target).
`NoConfinement` (Windows/BSD, `KIRI_SANDBOX=off`) passes the `Command` through unchanged, so it was never
affected — `run_shell`'s own scrub is the only guard there.

## Consequences

- `run_command`/hooks children can no longer read back any harness secret via `env`/`printenv` — matching
  the guarantee MCP server children already had.
- Locked by `scrub_env_keeps_only_the_allowlist` (unit test of the allowlist logic, via an injected lookup
  closure — no real env mutation, since edition-2024 `std::env::set_var` is `unsafe` and this crate forbids
  `unsafe`) and `run_shell_scrubs_env_down_to_the_allowlist` (end-to-end: spawns a real child, lists its
  actual environment, asserts every var is in the allowlist — catching anything the *test process's own*
  ambient environment might otherwise leak through). The confine-adapter regression is locked by
  `confine_does_not_leak_the_full_parent_env_into_the_wrapped_command` in both `macos.rs` and `linux.rs` —
  this test must spawn the real wrapped command and inspect its actual environment, not assert on
  `wrapped.as_std().get_envs()`: an earlier draft asserted on `get_envs()` directly and was vacuous (it
  still passed with `wrapped.env_clear()` deleted, since `get_envs()` can't see whether `env_clear()` was
  called either way — the exact blind spot the bug itself exploits). Verified the corrected, real-spawn
  version of the macOS test fails when `wrapped.env_clear()` is removed and passes when it is present,
  before landing it.
- A user's own shell environment (e.g. a personal `$EDITOR` or `$GOPATH`) is no longer visible to a
  `run_command`/hook child unless it happens to be one of the allowlisted vars — a deliberate fluidity
  trade-off for closing the credential-exposure surface, consistent with ADR 0022's precedent (network
  deny-by-default over an allow-list).
- Closes #25, #49.
