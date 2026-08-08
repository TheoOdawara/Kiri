# Kiri-V2 Design Documentation

This directory records decisions for the greenfield Kiri-V2 rewrite.

The current implementation under the repository root is a source of requirements,
known failures, and lessons learned. It is not the architecture for Kiri-V2.

## Current checkpoint

- [Session handoff](session-handoff.md)
- [Open decisions](open-decisions.md)

## Decisions

- [0001. Foundation stack](decisions/0001-foundation-stack.md)
- [0002. Harness configuration and discovery](decisions/0002-harness-configuration-and-discovery.md)
- [0003. Execution modes and policy enforcement](decisions/0003-execution-modes-and-policy-enforcement.md)
- [0004. Memory architecture and backends](decisions/0004-memory-architecture-and-backends.md)
- [0005. Live session control](decisions/0005-live-session-control.md)
- [0006. Primary user journey](decisions/0006-primary-user-journey.md)
- [0007. Complete Kiri-V2 v1 scope](decisions/0007-complete-v1-scope.md)
- [0008. TUI-first with a first-class CLI](decisions/0008-tui-first-cli-first-class.md)
- [0009. Windows-native execution is first-class](decisions/0009-windows-native-execution.md)
- [0010. Native Windows runtime and PowerShell 7](decisions/0010-native-windows-runtime-and-pwsh.md)
- [0011. Installer-managed PowerShell 7 prerequisite](decisions/0011-installer-managed-powershell.md)
- [0012. Package distribution and channel-owned auto-update](decisions/0012-package-distribution-and-auto-update.md)
- [0013. MSIX-managed PowerShell 7 prerequisite](decisions/0013-msix-managed-powershell.md)
- [0014. PowerShell 7 rolling stable update policy](decisions/0014-powershell-update-policy.md)

## Specifications

- [0001. On-disk schemas](specs/0001-on-disk-schemas.md)

Only decisions confirmed during the design process belong in the accepted ADRs.
Unresolved design areas remain undocumented until their blocking ambiguities are
resolved. Memory extraction heuristics and dependency behavior remain
implementation-level design work; the v1 on-disk schemas are defined in the
specification above.
