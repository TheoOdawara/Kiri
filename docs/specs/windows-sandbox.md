# Windows sandbox acceptance contract

## Goal

Ship Windows v1 through a native launcher and a confined WSL2 runtime, with mode behavior that is observable
and stable across upgrades.

## Non-goals

- Running the full Kiri runtime or user commands natively on Windows.
- Automatically installing WSL or completing a distribution's first-user setup.
- Hostname-restricted egress; ADR 0022 remains the active network policy.

## Acceptance criteria

- Given a Windows launch, when `kiri` starts, then the native process executes the matching Linux payload
  through an absolute `wsl.exe` path without a shell.
- Given no supported WSL2 distribution, when `kiri` starts, then it exits with the Ubuntu 24.04 install
  command and does not fall back to Win32 execution.
- Given Default mode, when a tool requests confirmation, then the human approval UI decides the call.
- Given Auto mode, when any action runs, then no human approval UI is called; risky or unconfined actions
  execute only after the isolated reviewer returns strict `allow`.
- Given Plan mode, when an admitted build or test runs, then the workspace mount is read-only and its
  writable caches are under the synthetic command home.
- Given Plan mode and an external file-tool target or command working directory, when the tool call is
  evaluated, then it is refused without a human action prompt. Commands still see the minimal read-only
  system roots required to execute inside the jail.
- Given `sandbox = off`, when Auto evaluates a call, then the guarantee set is empty and the reviewer sees
  every action.
- Given WSL NAT and a loopback local-provider URL, when the provider is built, then only the host component
  is replaced by the validated Windows gateway. Given mirrored networking or a remote URL, it is unchanged.
- Given `KIRI_SANDBOX=require`, when bubblewrap cannot enforce the requested policy, then command and hook
  execution fail closed.
- Given a release install, when either binary differs from `SHA256SUMS`, then installation stops before the
  launcher or payload is installed.

## Verification

Unit tests cover mode gates, strict reviewer parsing, launch argument construction, path translation,
launcher-state validation, NAT rewriting, guarantee matching, read-only bubblewrap arguments, environment
scrubbing, and command-home placement. CI runs format, Clippy with warnings denied, build, and all targets on
Ubuntu and Windows. A release cannot be considered verified until a supported real WSL2 distribution runs
the installer and `kiri wsl status` reports the payload and bubblewrap ready.
