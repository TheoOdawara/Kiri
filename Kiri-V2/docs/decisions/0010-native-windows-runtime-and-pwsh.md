# 0010. Native Windows runtime and PowerShell 7

- Status: accepted
- Date: 2026-08-04

## Context

Windows is a first-class Kiri-V2 target and must support Windows-native tools.
The runtime should not depend on WSL, because the product contract includes
commands and tools installed on the Windows host.

Kiri also needs one deterministic shell contract for compound commands and
scripts on Windows. Windows PowerShell 5.1 and PowerShell 7 are separate
products; PowerShell 7 is started with `pwsh` and is installed side by side
with the older Windows PowerShell.

## Decision

- Kiri-V2 uses a native Windows runtime. WSL is not required for the Windows
  runtime in v1.
- PowerShell 7, invoked as `pwsh`, is the canonical shell for shell execution
  on Windows.
- Structured execution continues to invoke programs directly when that is the
  appropriate contract; `pwsh` is used for PowerShell scripts and compound
  shell commands.
- `pwsh` runs inside the same policy, sandbox, credential, redaction, audit,
  timeout, and process-tree controls as every other command.
- Windows PowerShell 5.1 is not the default Kiri shell. Explicit legacy
  compatibility behavior remains an operational decision.

The installer-managed prerequisite is defined in ADR 0011 and uses the latest
stable PowerShell 7 release available at installation time. Discovery rules,
installation failure behavior, and update or pinning policy remain open.

## Consequences

- Windows execution has one native host/runtime boundary in the default path.
- Windows-native tools and PowerShell scripts remain available without a WSL
  bridge.
- Kiri must define and enforce a native Windows sandbox compatible with
  `pwsh`, child processes, Windows paths, and authorized tools.
- PowerShell 7 is an installer-managed Windows runtime prerequisite using the
  latest stable release available at installation time.

## Alternatives considered

- Making WSL the required Windows runtime was rejected because it would put a
  second operating-system boundary in front of an official Windows-native
  capability.
- Using Windows PowerShell 5.1 as the canonical shell was rejected because
  PowerShell 7 is the intended modern shell contract and is maintained
  separately from the Windows-shipped product.

## References

- [Install PowerShell 7 on Windows](https://learn.microsoft.com/en-us/powershell/scripting/install/install-powershell-on-windows)
- [Differences between Windows PowerShell 5.1 and PowerShell 7.x](https://learn.microsoft.com/en-us/powershell/scripting/whats-new/differences-from-windows-powershell)
