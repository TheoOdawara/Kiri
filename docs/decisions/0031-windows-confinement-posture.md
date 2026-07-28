# ADR 0031 — Windows write confinement: a write-restricted token, measured

- Status: Accepted
- Date: 2026-07-26
- Amends: ADR 0009 (`0009-os-command-sandbox.md`) and ADR 0018 — both left Windows on `NoConfinement`
  with "Windows is a later port" as the justification. v1 now ships macOS, Linux **and** Windows, so
  that justification is retracted; the gap was a v1 gap, not deferred work.
- Implemented by: `tools/infrastructure/confine/windows.rs` and its `restricted` submodule.
- Every claim below was measured on Windows 11, unelevated. Where a mechanism is rejected, it is
  rejected because it was tried and observed to fail, not because it was reasoned about.

## Context

`CommandSandbox` had three adapters: `MacosSeatbelt` (`sandbox-exec`), `BwrapSandbox` (`bwrap`), and
`NoConfinement`. `confine::detect()` selected `NoConfinement` on Windows, so `run_command` there ran a
child with the user's full token — every file the user could write, the child could write.

ADR 0030 ships a command deny-list on all three platforms, which made the Windows hole load-bearing: an
unlisted program now runs *silently* in auto mode. On macOS and Linux the OS layer catches what the
heuristic misses. On Windows, nothing did.

## Decision

**A write-restricted token (`CreateRestrictedToken` + `CreateProcessAsUserW`), unelevated.**

The command runs under a token built from the user's own, with:

- **`WRITE_RESTRICTED`** — restricting SIDs are evaluated on *write* access only. Without this flag the
  restricting set filters every access, and the confined process cannot read its own shell or the
  toolchain binaries: `cargo --version` exits 1 with no output, `git` cannot open `/dev/null`, and
  `pwsh` dies loading `BCrypt.dll`. This flag is the entire difference between "write confinement" and
  "nothing runs".
- **`DISABLE_MAX_PRIVILEGE`** — drops every privilege but `SeChangeNotify`, so the child cannot take
  ownership of a file to widen its own access back out.
- **Three restricting SIDs**: `RESTRICTED` (`S-1-5-12`), `Everyone`, and the **logon-session SID**.
  Each earns its place by what breaks without it. `Everyone` covers the ambient objects any process
  touches before reaching user code — the `NUL` device, the window station, the crypto provider. The
  logon-session SID covers `\Sessions\<n>\BaseNamedObjects`, without which `rustc` dies on
  `failed to create jobserver: Acesso negado`. Neither weakens the guarantee: files under the user's
  profile are granted to the *user* SID, which is in neither set.
- **An extended default DACL.** `SetTokenInformation(TokenDefaultDacl)` adds `RESTRICTED` to the token's
  default DACL, so every kernel object the confined process creates carries an ACE the restricting-SID
  check can pass. Without it the process cannot use a pipe **it created itself** — the new object
  inherits a DACL granting the user SID and SYSTEM but none of the restricting SIDs, so opening the
  other end fails with `Acesso negado`. That single omission broke `rustc` spawning the linker, `cargo`,
  and every shell pipeline: anything that captures a subprocess's output.

**Writable roots** get an inheritable ACE granting `RESTRICTED` Modify: the workspace, the configured
toolchain directories (`extra_rw`), and `%TEMP%` — the MSVC linker writes `lnk{…}.tmp` there, so without
it `cargo build` fails with `LNK1104: cannot open file`. `extra_ro` is deliberately not granted: reads
are not confined here, so a read-only grant would be a no-op that reads like a guarantee.

**The launcher is this binary re-executed** as `kiri confined-exec`. `CreateProcessAsUser` *is* the
spawn, but the `CommandSandbox` port decorates a `Command` rather than spawning one — that is what keeps
the single spawn site in `exec::run` with its timeout and tree-kill. So `confine()` rewrites the command
the way macOS rewrites it to `sandbox-exec` and Linux to `bwrap`; Windows simply ships no such launcher,
so Kiri is its own. The existing Job Object around the outer spawn already kills the grandchild, so a
timeout still reaps the real command. `main` dispatches this subcommand **before** `load_global_env`, so
the launcher inherits exactly the scrubbed environment its parent gave it and never seeds API keys into
a process whose whole job is to spawn an untrusted command.

### Rejected: AppContainer

The stronger primitive, and what Microsoft points at. Rejected because its **read** posture is
default-deny: a toolchain does not survive it. `cargo` reads `~/.cargo` and the rustup toolchain, every
compiler reads system headers and the CRT. Making a real build work means enumerating and granting each
of those, and each new toolchain is a new bug report that reads as "Kiri broke my build". A confinement
layer users disable is worth less than a weaker one they keep on. `opencode` chose AppContainer; that is
a defensible different trade-off, not a refutation of this one.

### Rejected: WSL

Considered because it brings a real Linux namespace and `bwrap`, which is already implemented. Rejected
on four counts, any one disqualifying:

1. **It does not confine the thing that needs confining.** A Windows workspace is reached through
   `/mnt/<drive>`, a DrvFs mount with no per-path write restriction from inside. The exact escape being
   closed stays open.
2. **It changes the toolchain under the user** — the command runs against Linux `cargo`/`node`, not the
   ones they installed.
3. **Path translation everywhere**, in both directions, including inside command strings the model
   composes — a new bug class in the highest-blast-radius surface the harness has.
4. **9P is slow** for the many-small-files access a build does.

It is also not universally installed, so it could never be more than an opportunistic upgrade — and an
opportunistic sandbox that silently does not apply is the failure mode ADR 0009 exists to avoid.

### Not adopted: the elevated model

Codex ships an **elevated** production sandbox: dedicated local users (`CodexSandboxOffline`/`Online`),
a helper crossing the UAC boundary, four processes in the chain, and firewall rules for network
isolation. It is stronger — it is the only one of these that denies network — and it is the shape to
grow into if network confinement becomes a requirement. It is out of scope here because it requires
elevation at install time and creates accounts on the user's machine.

## Consequences

Measured, not assumed. Under confinement: `git status`/`log`/`diff`, file writes, shell pipelines,
`rustc` (spawning and capturing the linker), `cargo --version`, and `cargo new` + `cargo build`
producing a runnable exe all succeed. Writes to the home directory, to another repo, and to `~/.kiri`
are denied; writes to the workspace succeed.

- **It stamps a persistent ACE on the user's directories.** The workspace, each toolchain dir, and
  `%TEMP%` keep the grant after the process, the session, and the install. Seatbelt and bwrap leave no
  trace; this does. It is intrinsic: without the ACE the confined child cannot write even the workspace.
- **A workspace the user does not fully own cannot be confined.** Stamping needs `WRITE_DAC`, which only
  the directory's real owner holds. A workspace on a secondary drive typically inherits its ACL from a
  root owned by Administrators and reaches the user through a group grant carrying no `WRITE_DAC` — this
  was reproduced on a real repo. `WindowsRestrictedToken::detect` probes for it at boot and reports the
  reason as a single `BootNotice` rather than letting every command die with the same error;
  `KIRI_SANDBOX=require` still refuses to run unconfined.
- **Reads are not confined.** A confined command can read `~/.kiri/credentials.json` — verified. This is
  weaker than macOS and Linux, where Seatbelt and bwrap shadow credential directories, and it is why the
  sensitive-path checks in `run_command` remain load-bearing on Windows.
- **Network is not denied.** `NetworkPolicy::Deny` is not enforced: that needs a per-process firewall
  rule, which needs elevation. Outbound requests were observed failing under the token, but that is an
  incidental effect of the write filter on crypto objects, not a guarantee, and it is not claimed as one.
- **A directory that already grants `Everyone` write cannot be blocked**, since `Everyone` is a
  restricting SID. This is the price of keeping the ambient objects reachable.
- **`RESTRICTED` is a well-known SID, not a Kiri-specific one.** Any other restricted process on the
  machine can write wherever Kiri granted it. A synthetic SID would need allocation, persistence, and a
  cleanup story; a well-known one needs none, and Windows already grants it read on the system
  directories, which is what lets a confined toolchain load its own DLLs.
