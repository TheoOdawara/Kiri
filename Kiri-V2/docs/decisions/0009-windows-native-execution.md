# 0009. Windows-native execution is first-class

- Status: accepted
- Date: 2026-08-04

## Context

Windows is a supported Kiri-V2 target. A Windows user may need the agent to
use tools and commands installed on the Windows host, not only Linux programs
inside a WSL distribution. Excluding those tools would make Windows support a
different and incomplete product.

The Windows runtime model is still open. It may be native, WSL-backed, or
hybrid, but that choice cannot silently remove Windows-native execution.

## Decision

Windows-native execution is a first-class Kiri-V2 v1 capability. It includes
using host commands and tools such as PowerShell, `cmd.exe`, Windows scripts,
and Windows-installed development tools when the user and policy authorize
them.

Any Windows runtime model must preserve this capability and route it through
the same policy, sandbox, credential, redaction, and audit contracts as every
other action. A bridge from WSL to the Windows host is an execution boundary,
not an escape hatch; host-side actions must be explicitly classified and
contained.

This decision does not select native Windows, WSL, or a hybrid runtime. It
sets a requirement that each candidate must satisfy.

## Consequences

- WSL-only execution with no controlled Windows-native capability is not a
  complete Windows product for Kiri-V2.
- The Windows design must account for host processes, Windows paths, process
  trees, credentials, network access, and sandbox enforcement.
- A runtime cannot be chosen only because it provides Bash compatibility or a
  convenient Linux sandbox.
- Windows-native support adds compatibility and security verification work to
  the v1 scope.

## Alternatives considered

- Supporting only Linux commands through WSL was rejected because it would make
  Windows-native tools unavailable to the agent.
- Treating Windows-native execution as an optional extension was rejected
  because it is part of the supported Windows product contract.
