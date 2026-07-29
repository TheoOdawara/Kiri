# TODO

## P0 — Windows release blockers

- [ ] Refuse any Bubblewrap workspace or extra mount that canonicalizes to `/`.
  - Keep the guard inside `BwrapSandbox`, where every caller is covered.
  - Cover startup at `\\wsl.localhost\<distro>\`, `/cd /`, `--bind / /`, `--ro-bind / /`, and a real
    WSLInterop attempt.
  - `host_interop=true` must never be reported when a Windows executable can run inside the jail.
  - Relevant code: `src/windows_launcher.rs:757`,
    `src/modules/tools/infrastructure/sandbox.rs:151`,
    `src/modules/tools/infrastructure/confine/linux.rs:133`.

- [ ] Restore DNS when `NetworkPolicy::Allow` is active under WSL2.
  - Mount the resolved WSL resolver file read-only at `/etc/resolv.conf`.
  - Add a real WSL check proving hostname resolution succeeds with network allowed and fails with network
    denied.
  - Relevant code: `src/modules/tools/infrastructure/confine/linux.rs:122`.

- [ ] Gate publication on the Windows acceptance contract.
  - Install the packaged Windows artifacts in a real supported WSL2 distribution.
  - Require a healthy `kiri wsl status` before GitHub Release or npm publication.
  - Make the publish job depend on this gate.
  - Relevant files: `.github/workflows/release.yml`, `docs/specs/windows-sandbox.md:37`.

## P1 — Required fixes

- [ ] Translate Windows CLI paths through the selected distribution.
  - Cover the CWD, `--path`, and `--instructions` without rewriting prompt text.
  - Use WSL path translation rather than assuming drives mount at `/mnt/<drive>`.
  - Cover spaces, Unicode, UNC paths, and a custom `automount.root`.
  - Relevant code: `src/windows_launcher.rs:757`, `src/windows_launcher.rs:782`.

- [ ] Make the Windows environment contract explicit and functional.
  - Either forward a validated allowlist of supported configuration variables to the payload or document
    and enforce `~/.kiri/.env`/config as the Windows path.
  - Cover `KIRI_PATH`, sandbox mode/network settings, and provider API-key import.
  - Never forward the full Windows environment.
  - Relevant code: `src/windows_launcher.rs:789`, `src/windows_launcher.rs:839`.

- [ ] Make Windows installation and upgrades transactional.
  - Validate WSL2 and `/etc/os-release` before any root mutation.
  - Probe `bubblewrap` and `iproute2`; run `apt` only when a dependency is missing.
  - Stage and verify the matching launcher and payload before activation.
  - Preserve or restore the previous working version on failure.
  - Relevant files: `packages/npm/install-windows.ps1`, `scripts/setup-windows-dev.ps1`.

- [ ] Bound launcher and sandbox preflights.
  - Add deadlines to WSL status/output calls that are expected to terminate.
  - Remove the redundant per-command Bubblewrap probe; the real absolute-path spawn must remain fail-closed.
  - Preserve process-tree cleanup.
  - Baseline observed on 2026-07-28: launcher approximately 1.20 s versus direct payload approximately
    0.16 s; Bubblewrap probe approximately 8 ms per command.
  - Relevant code: `src/windows_launcher.rs:809`,
    `src/modules/tools/infrastructure/confine/linux.rs:50`,
    `src/modules/tools/infrastructure/exec.rs:182`.

- [ ] Make `kiri wsl status` a reliable readiness probe.
  - Return non-zero when the payload is missing or Bubblewrap is missing/unusable while still printing the
    complete diagnosis.
  - Add tests for every degraded state.
  - Relevant code: `src/windows_launcher.rs:64`.

- [ ] Revalidate launcher state against the distribution's current user.
  - Detect a changed WSL `HOME` instead of executing a syntactically valid stale payload path.
  - Preserve the versioned payload invariant and direct the user to `kiri wsl repair`.
  - Relevant code: `src/windows_launcher.rs:155`, `src/windows_launcher.rs:360`.

- [ ] Provide a complete, idempotent Windows uninstall path.
  - Remove the selected WSL runtime, Windows payload cache, launcher, and the exact user `PATH` entry.
  - Define whether toolchain packages and command homes are retained or removed, and report that choice.
  - Relevant files: `packages/npm/package.json`, `packages/npm/install-windows.ps1`,
    `src/windows_launcher.rs:113`.

## P2 — Verification and cleanup

- [ ] Run `scripts/test-setup-windows-dev.ps1` in Windows CI and add installer tests for checksum failure,
  distro validation, rollback, conditional dependency installation, and uninstall.

- [ ] Keep `kiri wsl status` read-only: do not persist newly discovered state while inspecting status.

- [ ] Reject surplus lifecycle arguments such as `kiri wsl status typo`.

- [ ] Make `setup-windows-dev.ps1 -WhatIf` describe automatic distro selection when `-Distro` is omitted.

- [ ] Remove active comments that still describe Windows as `NoConfinement`, Windows/Linux as future
  targets, or Plan mode as prompting for admitted commands.
  - Relevant code: `src/modules/tools/application/command_sandbox.rs:63`,
    `src/modules/tools/infrastructure/sandbox.rs:490`,
    `src/modules/tools/application/tool.rs:151`.

## Verification gate

- [ ] Windows: `cargo fmt --check`.
- [ ] Windows: `cargo clippy --all-targets -- -D warnings`.
- [ ] Windows: `cargo build`.
- [ ] Windows: `cargo test --all-targets`.
- [ ] Linux payload in supported WSL2: run the same four gates.
- [ ] Run the Windows installer from packaged artifacts and observe a healthy `kiri wsl status`.
- [ ] Re-run the root-mount, DNS, host-interop, write-confinement, secret-protection, network-denial, and
  timeout probes against the real packaged runtime.
