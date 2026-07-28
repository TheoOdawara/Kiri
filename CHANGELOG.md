# Changelog

All notable changes to this project are documented here.

## Unreleased

### Added

- Windows launcher and versioned WSL2 runtime lifecycle (`status`, `use`, `repair`, `clean`, `remove`).
- Explicit sandbox guarantees, prompt-free Auto reviewer, and prompt-free read-only Plan execution.
- Synthetic per-workspace command homes and NAT/mirrored bridging for Windows-local providers.
- Windows installer, npm/Bun package, Windows/Linux CI, release checksums, and provenance attestations.

### Changed

- Windows v1 now runs the Linux runtime under WSL2 and bubblewrap instead of executing commands natively.
- Default is the only approval mode that asks the user.

### Removed

- Native Win32 restricted-token confinement, persistent workspace DACL mutation, and the crate's unsafe-code
  exception.
