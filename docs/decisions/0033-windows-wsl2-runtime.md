# ADR 0033 — Windows uses a WSL2 runtime with bubblewrap confinement

- Status: Accepted
- Date: 2026-07-28
- Supersedes: ADR 0031 and ADR 0032
- Amends: ADR 0018, ADR 0019, ADR 0030

## Context

The native restricted-token adapter from ADR 0031 confined some writes but did not confine reads or
network access. It also replaced directory DACLs persistently, could widen an absolute working directory,
and required the crate-wide unsafe lint to be weakened. Those properties do not meet the Windows v1
security contract.

Kiri already has a Linux `CommandSandbox` adapter based on bubblewrap. WSL2 can run that adapter while a
small native launcher preserves a normal `kiri` command in Windows terminals. WSL alone is not the sandbox:
the boundary is the bubblewrap namespace created inside WSL2.

Approval and sandboxing are separate axes. The user chooses Default, Auto, or Plan, and may explicitly set
the sandbox to `off`. The UI must describe the effective guarantees rather than infer safety from a single
enabled flag.

## Decision

### Runtime boundary

The Windows executable is a launcher only. It invokes the version-matched Linux payload with the absolute
`%SystemRoot%\System32\wsl.exe`, an argument vector, and a cleared Windows environment. It never evaluates a
shell string. Native Win32 command execution and the restricted-token adapter are removed; `unsafe` returns
to `forbid`.

Supported distributions are Ubuntu 22.04/24.04/26.04 and Debian 12/13 on WSL2. Docker's internal
distributions are never selected. The launcher stores the selected distribution and exact versioned payload
path under `%LOCALAPPDATA%\Kiri`, validates both on every launch, and supports `status`, `use`, `repair`,
`clean`, and `remove` lifecycle commands.

### Confinement

The Linux adapter does not bind the guest root. It binds only required system roots read-only, mounts the
workspace read-write for Default/Auto or read-only for Plan, mounts a synthetic per-workspace command home
read-write, and mounts explicitly configured extra roots. Each command receives fresh PID, IPC, mount, and
network namespaces as required, a new session, no capabilities, and dies with its parent.

The synthetic command home contains command caches. The real home, Kiri credentials, SSH agent variables,
and credential directories are not inherited or mounted. Toolchain executables are read-only by default;
cache writes go to the synthetic home.

`SandboxGuarantees` reports filesystem-read, filesystem-write, network-denial, process/IPC, host-interop,
and secret-protection capabilities explicitly. `KIRI_SANDBOX=require` checks the requested policy against
those guarantees. `kiri sandbox status` prints the effective values.

### Approval modes

- **Default** is the only mode that asks the user to approve tool calls.
- **Auto** never asks for human action approval. Deterministically safe actions run directly when OS confinement is present.
  Risky actions, and every action when the sandbox is explicitly off, go to an isolated provider reviewer.
  Arbitrary shell commands always go through that reviewer because shell text cannot be proven safe by a
  token classifier.
  Only a strict `allow` executes; deny, uncertainty, malformed output, timeout, or provider failure refuses.
  Explicit external paths are refused rather than silently widening the workspace.
- **Plan** never asks for human action approval. Only plan-safe tools and commands are offered, the physical workspace is
  mounted read-only, and external file-tool targets and command working directories are refused. Commands
  retain the minimal read-only system roots needed to execute. Builds and tests may write only to the synthetic home.
  The mode-independent runaway checkpoint remains active.

### Windows-local providers

The launcher reads `wslinfo --networking-mode`. Mirrored networking preserves loopback URLs. NAT mode reads
the IPv4 default gateway and passes it to the runtime. Provider construction rewrites only validated
`localhost`/loopback URLs to that gateway; remote endpoints and invalid bridge values are unchanged.

### Distribution

Windows releases contain a native launcher, a matching Linux x86-64 payload, SHA-256 checksums, a PowerShell
installer, and GitHub build-provenance attestations. The installer validates version and distribution,
verifies both binaries, installs `bubblewrap`, caches the Linux payload for repair, and adds the launcher to
the user PATH. The npm/Bun package runs the same PowerShell installer; Bun requires explicit package trust.

## Consequences

- Windows gets the same read/write/network/process confinement primitive as Linux, and the unsafe Win32
  surface disappears.
- Windows-native compilers and shells are intentionally not used. Projects opened through `/mnt/<drive>`
  retain DrvFs performance; projects stored in the WSL filesystem are faster and are supported through
  `\\wsl.localhost\<distro>\...` working directories.
- WSL2 and bubblewrap are hard prerequisites. There is no silent native fallback.
- The default network posture remains ADR 0022's deny/allow session policy. A hostname-restricted egress
  proxy is not part of this decision and must be authorized and reviewed as a separate boundary change.

## Alternatives considered

- **Keep the restricted token:** rejected because it persistently mutates DACLs and cannot claim read or
  network confinement.
- **AppContainer:** rejected because making arbitrary developer toolchains usable requires broad capability
  grants and a separate privileged setup path.
- **Dedicated Windows users and firewall rules:** stronger but requires elevation and permanent host state.
- **WSL without bubblewrap:** rejected; WSL is a compatibility VM, not a per-command sandbox.
