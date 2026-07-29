# Windows WSL2 runtime hardening plan

Status: planned; implementation intentionally deferred.

This plan closes the Windows release and runtime gaps identified in `TODO.md`. The checklist in
`TODO.md` is the source of truth for individual acceptance items; this file records the execution order
for the next work session.

## 1. P0 release blockers

1. Make `BwrapSandbox` reject `/` as the workspace or any extra mount after canonicalization, including
   WSL-root and bind-root inputs. Add regression tests for workspace `/`, `--bind / /`, and `--ro-bind / /`.
2. Restore WSL DNS for `NetworkPolicy::Allow` by mounting the resolved `/etc/resolv.conf` read-only. Verify
   hostname resolution with networking allowed and failure when networking is denied.
3. Add a real Windows acceptance gate to release publication. Install the packaged artifacts in supported
   WSL2, require a healthy `kiri wsl status`, and make GitHub Release/npm publication depend on that gate.

## 2. P1 required runtime fixes

1. Translate Windows paths through the selected distribution using `wslpath`; cover CWD, `--path`, and
   `--instructions` without rewriting prompt text, including spaces, Unicode, UNC paths, and custom mount
   roots.
2. Define a validated Windows environment allowlist and forward only supported configuration values, or
   enforce the documented `~/.kiri/.env`/config path. Never forward the complete Windows environment.
3. Make installation and upgrades transactional: validate WSL2/release first, install missing dependencies
   only, stage and verify matching artifacts, activate atomically, and restore the previous version on
   failure.
4. Bound launcher/sandbox preflight calls with deadlines, remove the redundant per-command Bubblewrap
   probe, and keep the absolute-path spawn fail-closed with process-tree cleanup.
5. Make `kiri wsl status` read-only and return non-zero for missing or unusable payload/Bubblewrap while
   printing the complete diagnosis.
6. Revalidate the stored launcher state against the distro's current `HOME` and direct stale state to
   `kiri wsl repair`.
7. Implement a complete, idempotent Windows uninstall covering runtime, payload cache, launcher, and the
   exact user `PATH` entry, with an explicit retention policy for toolchains and command homes.

## 3. P2 verification and cleanup

1. Extend Windows installer/setup tests for checksum failure, distro validation, rollback, conditional
   dependencies, uninstall, automatic distro selection, and rejected surplus lifecycle arguments.
2. Keep `kiri wsl status` free of state writes and update stale comments/documentation that contradict the
   current Windows confinement and Plan-mode behavior.
3. Run the Windows and supported-WSL2 verification gates from `TODO.md`, including the packaged-runtime
   acceptance probes for root mounts, DNS, host interop, write confinement, secret protection, network
   denial, and timeouts.

## Definition of done

The plan is complete only when `cargo fmt --check`, Clippy with warnings denied, build, and all tests pass
on Windows and in the Linux payload, the packaged installer is exercised in a supported WSL2 distribution,
and `kiri wsl status` reports a healthy runtime without mutating state.

Vault: the Kiri project note confirms that Windows is a v1 target and that the existing sandbox adapter
abstraction should be retained for the WSL2 implementation.
