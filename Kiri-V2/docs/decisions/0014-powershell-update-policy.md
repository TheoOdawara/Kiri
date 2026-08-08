# 0014. PowerShell 7 rolling stable update policy

- Status: accepted
- Date: 2026-08-08
- Amends: 0011, 0013

## Context

Kiri-V2 installs PowerShell 7 as a signed MSIX or MSIXBundle, while the
package owner remains responsible for servicing that installation. An exact
Kiri-managed version pin would conflict with MSIX, Windows Update, and WinGet
ownership and would require Kiri to maintain download, rollback, and cache
behavior for a dependency it does not own.

Kiri still needs a predictable runtime contract: preview builds must not enter
the supported path, and a session must not silently change its shell after it
has started.

## Decision

PowerShell 7 follows a rolling stable policy with no exact Kiri-managed version
pin.

- The Kiri installer installs the latest stable PowerShell 7 package available
  through the official MSIX distribution at installation time.
- Subsequent updates are owned by Microsoft's supported update channel for the
  installation, such as Microsoft Update, WinGet, or the registered MSIX
  channel. Kiri does not run a PowerShell upgrade command and never overwrites
  the installed package.
- Kiri accepts only a valid, supported, stable PowerShell 7 executable. Preview
  builds and versions outside Microsoft's support policy are rejected for the
  supported Windows shell path.
- Kiri resolves and validates `pwsh` when a session starts, records the
  concrete executable and version for that session, and keeps that resolution
  fixed until the session ends.
- If an update check or package-owned update is unavailable, Kiri continues
  with the currently installed version when it remains valid. Kiri reports an
  actionable diagnostic when no valid PowerShell 7 executable is available.

The exact supported-version floor and any future LTS-only policy require a
separate decision if the rolling stable contract becomes insufficient.

## Consequences

- Kiri remains compatible with package-manager ownership and does not create a
  second update authority for PowerShell.
- A session is reproducible with respect to its resolved `pwsh` executable,
  even if Windows installs an update while Kiri is running.
- Different sessions may use different supported PowerShell 7 servicing
  versions over time.
- Kiri must expose the resolved PowerShell path and version in diagnostics or
  session metadata without logging sensitive environment data.

## Alternatives considered

- **Pin an exact PowerShell build in Kiri:** rejected because it conflicts with
  MSIX update ownership and adds a dependency lifecycle Kiri would have to
  maintain.
- **Pin to one LTS line:** rejected for now because Kiri's accepted prerequisite
  contract is the latest stable release, and no LTS-only requirement has been
  established.
- **Run `winget upgrade` at every Kiri startup:** rejected because it adds
  elevation, network, latency, and package-manager side effects to session
  startup.

## References

- [Install PowerShell 7 on Windows](https://learn.microsoft.com/en-us/powershell/scripting/install/install-powershell-on-windows)
- [PowerShell Support Lifecycle](https://learn.microsoft.com/en-us/powershell/scripting/install/powershell-support-lifecycle)
