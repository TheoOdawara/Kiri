# ADR 0031 — Windows confinement posture for v1: a restricted token, not AppContainer or WSL

- Status: Accepted
- Date: 2026-07-25
- Amends: ADR 0009 (`0009-os-command-sandbox.md`) and ADR 0018 — both left Windows on `NoConfinement`
  with "Windows is a later port" as the justification. v1 now ships macOS, Linux **and** Windows, so
  that justification is retracted; the gap is a v1 gap, not deferred work.
- Implemented by: the `WindowsRestrictedToken` adapter (phase 7 of this effort). This ADR records the
  choice and its known holes before the code lands, so the limits are declared rather than discovered.

## Context

`CommandSandbox` has three adapters: `MacosSeatbelt` (`sandbox-exec`), `BwrapSandbox` (`bwrap`), and
`NoConfinement`. `confine::detect()` selects `NoConfinement` on Windows, so `run_command` there runs a
child with the user's full token — every file the user can write, the child can write.

ADR 0030 ships a command deny-list on all three platforms, which makes the Windows hole load-bearing:
an unlisted program now runs *silently* in auto mode, with nothing underneath it. On macOS and Linux the
OS layer catches what the heuristic misses. On Windows, nothing does.

Three mechanisms were evaluated.

## Decision

**Elected: a restricted token (`CreateRestrictedToken` + `CreateProcessAsUser`), unelevated.**

The child is spawned with deny-only SIDs and a synthetic SID that the workspace root and the configured
writable roots (`SandboxPolicy::extra_rw`, which already carries exactly that list) grant write access
to via an explicit ACE. Read access is left broad — the toolchain needs it — and write is the axis
being confined. No admin rights, no dedicated user accounts, no service installation: a harness a user
installs must not require an elevated prompt to be safe by default.

This is the same mechanism Codex CLI settled on for unelevated Windows, and its precedent matters here:
the alternatives below are not theoretical rejections, they are a path already walked.

### Rejected: AppContainer

AppContainer is the stronger primitive — a real security boundary with per-capability grants — and it is
what Microsoft points at for sandboxing. It is rejected because its **read** posture is default-deny.
A toolchain does not survive that: `cargo` reads `~/.cargo` and the rustup toolchain, `node` reads the
global prefix, every compiler reads system headers and the CRT. Making a real build work inside an
AppContainer means enumerating and granting each of those paths, and each new toolchain is a new bug
report that reads as "Kiri broke my build". A confinement layer users disable is worth less than a
weaker one they keep on.

### Rejected: WSL

Running commands inside WSL was considered because it brings a real Linux namespace — and `bwrap`, which
is already implemented. It is rejected on four counts, any one of which is disqualifying:

1. **It does not confine the thing that needs confining.** A Windows workspace is reached through
   `/mnt/f`, a DrvFs mount with no per-path write restriction from inside. The exact escape being closed
   — writing outside the workspace on the Windows filesystem — stays open.
2. **It changes the toolchain under the user.** The command the user typed would run against Linux
   `cargo`/`node`, not the ones they installed. Build artifacts, paths, and line endings all diverge from
   what the same command produces outside Kiri.
3. **Path translation everywhere.** Every path crossing the boundary needs `C:\x` ⇄ `/mnt/c/x`
   rewriting, in both directions, including inside command strings the model composes. That is a new
   class of bug in the highest-blast-radius surface the harness has.
4. **9P is slow.** Cross-filesystem I/O over `/mnt` is an order of magnitude slower than native for the
   many-small-files access a build does.

WSL is also not universally installed, so it could never be more than an opportunistic upgrade — and an
opportunistic sandbox that silently does not apply is the failure mode ADR 0009 exists to avoid.

## Consequences

Declared up front, not discovered later:

- **A directory that already grants write to `Everyone` cannot be blocked.** The classic case is `%TEMP%`.
  A deny-only SID cannot revoke an access an ACE grants to a group the token still carries.
- **There is no network denial.** `NetworkPolicy::Deny` is not honored on Windows: enforcing it would
  need a per-process firewall rule, which needs elevation. `supports_confinement()` and the system
  prompt must say what is actually enforced — claiming a network guarantee the OS layer does not provide
  is security theater, which this project treats as a defect.
- **Write confinement is the only guarantee.** Reads are not confined, so the sensitive-path heuristics
  in `run_command` remain the only thing standing between a command and `~/.aws/credentials` on Windows.
  That is weaker than macOS/Linux, and it is why `KIRI_SANDBOX=require` remains meaningful.
- Implementing this needs `unsafe` for the Win32 calls, which the crate currently forbids outright.
  Relaxing `unsafe_code` from `forbid` to `deny` — with `#[allow(unsafe_code)]` confined to this one
  module — is a separate decision and gets its own ADR, because the crate-wide guarantee exists on
  purpose and ADR 0027 leaned on it as an argument.
