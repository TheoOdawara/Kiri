# Kiri-V2 design checkpoint

- Date: 2026-08-08
- Phase: design only
- Implementation status: no Kiri-V2 code has been changed

This is the handoff point for continuing the design on another machine. The
authoritative checklist is [open-decisions.md](open-decisions.md); accepted
decisions are linked from the [design index](README.md).

## Current position

The work is in `1. Product boundary and v1 scope`.

The current product contract says:

- v1 means the complete Kiri-V2 product, not a reduced MVP;
- the primary flow starts with `kiri` in the target directory;
- the user trusts the directory, starts immediately, changes modes freely, and
  supervises a live session;
- the TUI is the primary interface, while the CLI is first-class for automation,
  headless use, direct actions, and agent invocation;
- CLI-started sessions, including child-agent sessions, remain visible and
  controllable in the TUI;
- Linux, macOS, and Windows are supported targets, with native runtimes;
- Windows-native commands and tools are first-class capabilities;
- Windows uses a native runtime and does not require WSL in v1;
- PowerShell 7 `pwsh` is the canonical Windows shell;
- the Kiri installer installs the latest stable PowerShell 7 release;
- new Windows installations use Microsoft's signed MSIX or MSIXBundle
  distribution, preferably through WinGet;
- installation requests elevation immediately, makes three total attempts with
  waits of 1 second and 3 seconds, then explains the resolution if it fails;
- `pwsh` discovery checks `PATH`, the registered MSIX installation, and then
  the legacy MSI directory for existing MSI installations;
- Kiri is distributed through npm/Bun on Windows, npm/Homebrew on macOS, and
  npm/AUR on Linux, with channel-owned automatic updates on every startup.

## Immediate next decision

Define PowerShell 7 update or pinning behavior after installation, then decide
whether local keyless providers are first-class in v1 and which remote providers
are supported. The distribution matrix is fixed in ADR 0012.

## Remaining product-boundary decisions

- PowerShell 7 update or pinning behavior after installation;
- whether local keyless providers are first-class in v1;
- which remote providers are supported in v1;
- what Claude/Codex/other harness compatibility means;
- the native Windows sandbox implementation.

Do not start implementation from this checkpoint. Continue resolving the
unchecked questions in `open-decisions.md` and record accepted choices as ADRs
under `Kiri-V2`.
