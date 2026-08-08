# 0013. MSIX-managed PowerShell 7 prerequisite

- Status: accepted
- Date: 2026-08-08
- Amends: 0011

## Context

ADR 0011 selected the official PowerShell 7 MSI as the Windows installer
mechanism. Microsoft now distributes PowerShell through MSIX as the default
WinGet package on supported Windows targets, and the installation guidance
states that PowerShell 7.7 will not have an MSI package. Keeping MSI as Kiri's
fixed mechanism would make the Windows prerequisite incompatible with future
PowerShell releases.

Kiri-V2 supports Windows 11 on both `x86_64` and `arm64`. The prerequisite
must therefore use the package format and installation identity that Microsoft
supports across those targets.

## Decision

Kiri-V2 installs the stable PowerShell 7 prerequisite as a signed MSIX or
MSIXBundle from Microsoft's official distribution.

- WinGet with the official `Microsoft.PowerShell` package is the preferred
  acquisition path.
- If WinGet is unavailable, the installer may use an official signed MSIX or
  MSIXBundle with Windows AppX tooling. It must not use an arbitrary mirror or
  third-party package source.
- Kiri does not select MSI for new installations. A valid pre-existing MSI
  installation may still satisfy the runtime prerequisite when its `pwsh`
  executable passes normal discovery and version validation.
- Existing retry count, retry delays, elevation timing, and failure reporting
  from ADR 0011 remain unchanged.
- PowerShell update and version-pinning behavior remains a separate decision.

`pwsh` discovery is ordered as follows:

1. a valid executable resolved from `PATH`;
2. the registered MSIX/App Execution Alias installation;
3. the standard MSI installation directory, only for compatibility with an
   existing MSI installation.

The resolver returns the concrete executable and validates its reported
PowerShell 7 version before a Windows session uses it. It must not assume that
all MSIX installations live in the legacy MSI directory.

## Consequences

- New Windows installations follow Microsoft's current package direction and
  remain compatible with PowerShell releases that no longer publish MSI.
- The installer and runtime need MSIX-aware package discovery in addition to
  ordinary executable lookup.
- MSIX package identity, signature, architecture, and version must be
  validated before installation is considered successful.
- Existing MSI installations remain a compatibility path, but MSI is not a
  future Kiri installation contract.

## Alternatives considered

- **Keep MSI as the only mechanism:** rejected because Microsoft has announced
  that future PowerShell releases will not publish MSI packages.
- **Use a ZIP distribution:** rejected because it would move installation,
  identity, and update ownership into Kiri instead of using Microsoft's
  supported Windows package model.
- **Require Microsoft Store only:** rejected because it would make the
  prerequisite dependent on one channel and would not cover all supported
  installation environments.

## References

- [Install PowerShell 7 on Windows](https://learn.microsoft.com/en-us/powershell/scripting/install/install-powershell-on-windows)
- [What is MSIX?](https://learn.microsoft.com/en-us/windows/msix/overview)
