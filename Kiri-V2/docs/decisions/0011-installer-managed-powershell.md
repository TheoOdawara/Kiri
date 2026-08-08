# 0011. Installer-managed PowerShell 7 prerequisite

- Status: accepted
- Date: 2026-08-04
- Amended by: 0013

## Context

PowerShell 7 is the canonical Windows shell for Kiri-V2. Requiring users to
discover and install it manually would make the Windows installation fragile
and leave the runtime contract incomplete.

## Decision

The Kiri installer automatically installs the latest stable PowerShell 7
release available at installation time as a required Windows runtime
dependency. A normal Kiri installation is not considered complete on Windows
until the `pwsh` dependency is available.

If installing or verifying `pwsh` fails, the installer retries internally. If
the retry still fails, it warns the user and explains the concrete steps needed
to resolve the dependency before installation can complete.

The installer makes three attempts in total: the initial attempt, a retry after
1 second, and a final retry after 3 seconds.

After installation, Kiri first resolves a valid PowerShell 7 `pwsh` executable
from `PATH`. If it is not available there, Kiri falls back to the standard
PowerShell 7 installation directory.

The installer requests elevation immediately when it is invoked, before it
starts the installation steps that require elevated access.

The Windows npm and Bun distribution packages invoke the official PowerShell 7
installer to install the required `pwsh` dependency. The installer mechanism
and package discovery are amended by ADR 0013, which selects signed MSIX or
MSIXBundle distribution. Kiri remains a separate native package artifact.

The PowerShell update or pinning policy after installation remains an
operational decision.

## Consequences

- Windows installation has one controlled prerequisite step for the canonical
  shell.
- Kiri cannot silently assume that `pwsh` is already present on a new Windows
  installation.
- The installer must report dependency-installation failures clearly and leave
  the installation in a known state.
- Runtime startup still verifies that the expected `pwsh` executable is
  available; installation-time success is not treated as proof forever.
- npm and Bun installations must handle the package installation's elevation
  and verification failure paths without leaving Kiri in an apparently
  complete state.

## Alternatives considered

- Requiring a manual PowerShell 7 installation was rejected because it adds
  avoidable setup friction and configuration drift.
- Bundling an unmanaged private copy inside the Kiri executable was rejected as
  the default because PowerShell has its own supported installation and update
  lifecycle.
- Using a package-manager-specific PowerShell installer was rejected because
  it would make the required Windows shell vary by Kiri distribution channel.
