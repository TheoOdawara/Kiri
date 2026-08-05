# 0012. Package distribution and channel-owned auto-update

- Status: accepted
- Date: 2026-08-05

## Context

Kiri-V2 supports native runtimes on Windows, macOS, and Linux. Users need
installation paths that fit each platform while receiving the same prebuilt
native executable without compiling Rust during installation.

Automatic updates are part of the product contract, but package managers must
remain the authority for the files they install. GitHub Releases provides the
release artifacts and the release-level flag that makes a version eligible for
automatic adoption.

## Decision

Kiri-V2 distributes one native package artifact per supported operating-system
and CPU target through these channels:

| Operating system | Official channels |
| --- | --- |
| Windows | npm and Bun |
| macOS | npm and Homebrew |
| Linux | npm and AUR |

Each operating system has `x86_64` and `arm64` artifacts. 32-bit targets are
not supported in v1.

The minimum operating-system policy is:

- Windows: any Windows 11 release still supported by Microsoft; Windows 10 is
  not supported;
- macOS: the current major release and the two preceding major releases,
  subject to the operating system's support for the target architecture;
- Linux: distributions providing `glibc >= 2.31`; the AUR channel follows the
  supported Arch Linux environment.

npm and Bun consume the same package distribution path. They do not compile
Kiri during installation. The native package artifact is the same release
artifact used by the platform-native package channels.

Each installation channel owns updates through its own package-manager path:

- npm installations update through npm;
- Bun installations update through Bun;
- Homebrew installations update through Homebrew;
- AUR installations update through the user's AUR package-manager path.

Kiri does not overwrite a binary owned by another package manager. GitHub
Releases is the distribution source and release control plane; a release must
be marked eligible by its GitHub release flag before channel update metadata
may adopt it.

Every Kiri startup checks for an eligible update. When one exists, Kiri applies
the update through the detected installation channel before opening the new
session, then restarts once into the updated version. If checking or applying
the update fails, Kiri starts with the currently installed version.

## Consequences

- Users receive automatic updates without a separate opt-in setting.
- Package-manager ownership remains intact and package metadata stays
  consistent with installed files.
- Startup can take longer when an update check or package-manager operation is
  required.
- The updater must detect the installation channel and fail safely when that
  channel is unavailable.
- Every artifact still requires integrity and authenticity verification before
  installation.

## Alternatives considered

- A Kiri self-updater that replaces package-manager-owned files was rejected
  because it creates package-manager drift and can be overwritten later.
- A single universal updater was rejected because it would bypass the native
  installation contract of each channel.
- Compiling Rust during npm or Bun installation was rejected because it makes
  installation depend on a local toolchain and increases failure surface.
