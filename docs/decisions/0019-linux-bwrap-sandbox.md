# ADR 0019 — Linux OS-level command sandbox via Bubblewrap; Windows still deferred

- Status: Accepted
- Date: 2026-07-02

## Context

ADR 0009 shipped OS-level confinement for `run_command` on macOS only (a generated Seatbelt profile via
`sandbox-exec`), and designed a Linux adapter around the `landlock` crate (FS path-beneath + ABI-v4
TCP-connect deny, with a `bwrap --unshare-net` fallback) but deferred it — "it needs a Linux host to
verify, and shipping unverified kernel-confinement code would violate run-to-verify." Windows had no
adapter at all. Both platforms fell back to `NoConfinement`: the path-policy chokepoint and the mandatory
`run_command` confirmation remained the only guards, and ADR 0009 tracked the gap as `security-debt`.

This ADR (part of the native-cross-platform-support initiative, see ADR 0018) closes the Linux half of
that debt and re-evaluates the Landlock-first design in light of a decision made while implementing it.

## Decision

**Linux: Bubblewrap (`bwrap`) launcher, not Landlock.** `tools/infrastructure/confine/linux.rs` adds
`BwrapSandbox`, an argv-transform adapter that is the direct structural sibling of the macOS Seatbelt
adapter: it rebuilds the already-constructed `tokio::process::Command` as
`bwrap <flags> -- <program> <args…>`, preserving cwd/env, and returns the wrapped command from `confine()`
without ever spawning it itself (the single spawn site in `exec::run` is untouched, matching ADR 0009's
port contract).

This reverses ADR 0009's stated Linux plan (`landlock`-first). The reason: Landlock is a deny-by-default
allow-list — expressing "read everything except `~/.ssh`" means enumerating every other path in the
filesystem rather than punching a hole in a permissive base, which is the opposite shape from the
macOS/Windows adapters and from the path-policy layer's own posture. Confining only the spawned child
(not the whole `kiri` process) additionally needs either a `pre_exec` hook (`unsafe`, ruled out — no
adapter needs `unsafe_code` today) or its own launcher binary — at which point it is strictly more
implementation than bwrap for a worse semantic match, before even accounting for Landlock's network-deny
ruleset needing kernel ≥6.7. Bubblewrap needs no Cargo dependency, no FFI, and no `unsafe` — same as the
macOS adapter — and reuses the identical last-match-wins mental model (bwrap applies bind mounts in
argument order; a later bind at the same path shadows an earlier one, exactly like Seatbelt's SBPL rule
ordering).

**Shape**, mirroring `build_profile`'s SBPL ordering:

1. **Permissive base**: `--ro-bind / /` (whole filesystem, read-only) + `--dev /dev` + `--proc /proc` +
   `--tmpfs <temp_dir>` — matches Seatbelt's `(allow default)` posture; the path-policy and confirmation
   layers remain the primary guard.
2. **Writes re-opened**: `--bind <root> <root>`, then `--bind <extra> <extra>` for each configured/
   per-call `extra_rw` (G2 — a write anywhere else surfaces `EROFS` from the read-only base).
3. **Credential shadows**, emitted after step 2 so they win even when the workspace root is a home
   ancestor (e.g. after `/cd ~`): `--tmpfs` over each single-sourced `SECRET_DIRS` entry and the harness's
   own `~/.kiri`, `--ro-bind /dev/null` over each `HOME_SECRET_FILES` entry (G3).
4. **Explicit read re-allows last**: `--ro-bind <extra> <extra>` for each configured `extra_ro`, so a
   legitimate read (e.g. `~/.aws/config`) can punch back through a credential shadow, same as Seatbelt's
   `extra_ro` handling.
5. `--unshare-net` only when `policy.network == Deny` (G1); always `--die-with-parent` (so `exec::run`'s
   `kill_on_drop`/timeout still reaps it) and `--chdir <cwd>` (bwrap resets cwd to `/` by default
   regardless of the parent process's own cwd, so this is required, not cosmetic).

**`detect()` probes, it does not trust `PATH`.** Ubuntu 24.04+ ships `bwrap`, but AppArmor can block
unprivileged user namespaces (`kernel.apparmor_restrict_unprivileged_userns=1`), which makes an installed
`bwrap` fail at runtime while still resolving on `PATH`. `detect()` and every `confine()` call instead run
`bwrap --ro-bind / / --unshare-net --dev /dev -- /bin/true` and require exit 0, the same fail-closed
re-check pattern the macOS adapter uses for `sandbox-exec`'s presence.

**Known, stated gap:** bwrap has no name-regex facility, so Seatbelt's `*.pem`/`.env` sensitive-name
read-denies (`push_sensitive_name_denies`) have no bwrap equivalent — only whole-directory/file shadowing.
Mitigated, not closed, by the file tools' `SensitiveMatcher` layer and `run_command`'s
always-decline-by-default confirmation; a real platform asymmetry, stated rather than silently dropped
(consistent with ADR 0009's own disclosure norm).

`SandboxPolicy`'s fields and the `secret_paths` constants (`HOME_SECRET_FILES`, `HARNESS_PRIVATE_DIR`)
were `#[cfg_attr(not(target_os = "macos"), allow(dead_code))]`-gated to a single consumer; both widen to
`not(any(target_os = "macos", target_os = "linux"))` now that two adapters share them.

**Windows: still deferred, not part of this ADR.** ADR 0018's plan identifies the only viable shape on
stable Rust — a separate `kiri-confine.exe` launcher binary doing AppContainer setup via Win32 FFI, since
`CommandExt::raw_attribute` is nightly-only — which needs exactly one scoped `#[allow(unsafe_code)]` (pre-
authorized by ADR 0009, never exercised yet). That trade-off has not been confirmed with the user, so
Windows `run_command` remains `NoConfinement` (confirmation-only), tracked as the remaining half of ADR
0009's `security-debt`.

## Consequences

- New `tools/infrastructure/confine/linux.rs`; `confine.rs` gains a `#[cfg(target_os = "linux")]` wiring
  arm in `default_command_sandbox`, mirroring the macOS arm exactly.
- `run_command.rs` gains a Linux counterpart to the macOS end-to-end confinement tests
  (`confined_run_command_cannot_write_outside_root`, `_still_works_inside_root`, plus a credential-read
  probe `confined_run_command_cannot_read_credential_dir`), each skip-green via `BwrapSandbox::detect()`
  when bwrap/userns is unavailable on the host — the same pattern the macOS tests use for
  `MacosSeatbelt::detect()`.
- **Cannot run-to-verify on the Windows development host used for ADR 0018.** The crate's `keyring`
  dependency pulls in a `libdbus-sys` build script that needs a Linux sysroot with `libdbus-1-dev` +
  `pkg-config` to cross-compile-check from Windows, and this host's WSL2 Debian distro has no Rust
  toolchain and no passwordless `sudo` to install one non-interactively — so `cargo check --target
  x86_64-unknown-linux-gnu` fails on that unrelated dependency before reaching this adapter's own code.
  The code was hand-reviewed against the macOS adapter's structure and bwrap's documented flag semantics
  instead; real verification is the existing `ubuntu-latest` CI leg (unaffected by the Windows
  cross-compile gap) plus a follow-up local WSL2 smoke test once a toolchain is provisioned there.
- **Closes** ADR 0009's Linux `security-debt`. **Leaves open** the Windows OS-enforcement debt — unchanged
  from ADR 0009, pending the `#[allow(unsafe_code)]` trade-off decision.
