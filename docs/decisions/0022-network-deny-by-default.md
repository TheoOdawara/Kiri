# ADR 0022 — Network deny-by-default for `run_command`

- Status: Accepted
- Date: 2026-07-05
- Amends: ADR 0009 (OS-level command sandbox)

## Context

ADR 0009 gave `run_command` a "dev-friendly, not all-or-nothing" network stance: denied by default, but
auto-**allowed** whenever the command's leading program matched a hardcoded allow-list of dev/package
tools (`cargo`, `npm`, `git`, `pip`, `go`, `make`, …). The grant was disclosed in the confirmation prompt
(informed consent), and chaining/substitution/backgrounding were hardened so a second program couldn't
inherit the leading program's grant (SEC-02 and follow-ups).

Audited as issue #5: the allow-list is still **leading-program-based**. An allow-listed tool's own
build/post-install script runs with network and can exfiltrate — `npm install` legitimately runs
arbitrary `postinstall` scripts from third-party packages, `cargo build` runs `build.rs`, both with the
same network grant as the tool itself. A malicious or compromised dependency does not need to smuggle a
second command past the chaining guard; it only needs the *already-granted* network of the tool that
loads it. The allow-list also does nothing to distinguish `cargo build` (compiles a workspace) from
`cargo publish` (uploads to a registry) — the whole leading program is one bucket.

The ideal fix — per-host egress filtering, so only known registry hosts (`crates.io`,
`registry.npmjs.org`, `pypi.org`, …) are reachable — is not implementable in the current sandbox layer:

- **macOS (Seatbelt/SBPL):** `network*` in the profile is all-or-nothing. SBPL has no hostname-aware
  filter primitive; a `(deny network-outbound (remote ip ...))` rule can match an IP/port, not a DNS name,
  and a build tool's registry endpoint resolves to CDN IPs that change without notice — an IP allow-list
  would be both wrong (blocks legitimate traffic) and stale within days.
- **Linux (bwrap `--unshare-net` / Landlock):** the ADR 0009 Linux adapter's network control is a network
  *namespace* unshare — namespace-level, not per-connection; Landlock's ABI (as adopted here) covers
  filesystem paths and TCP connect-by-port, not by hostname.

Neither adapter can express "allow DNS + TCP to these registry hosts only" without a userspace egress
proxy (a new component, its own trust surface, and its own maintenance burden) or an OS-native filter this
project's dependencies do not provide. Building that proxy is out of scope for this ADR.

## Decision

**Remove the command-name-based network allow-list. `run_command`'s network stance is now exactly the
sandbox's base stance — deny by default, with no per-command widening.**

- `RunCommand::network_for` (the leading-program-match logic) and `DEFAULT_NET_ALLOW` /
  `KIRI_SANDBOX_NET_ALLOW_CMDS` are deleted outright, not deprecated. `execute()` now passes
  `sandbox.network()` straight through to `command_policy`.
- The confirmation's network-grant disclosure is deleted with it — there is nothing left to disclose,
  since no command ever gets a grant it wasn't already given at the session level.
- **Session-wide opt-in, unchanged:** `KIRI_SANDBOX_NETWORK=allow` still widens `run_command`'s network
  for the whole session (this was already a coarser, honest, explicit control — ADR 0009 introduced it
  alongside the now-removed allow-list, and it needs no change here). A user who wants `cargo build` /
  `npm install` to work sets this once for the session; the harness no longer decides it silently per
  command.
- **Per-host egress filtering: won't-fix on the current sandbox stack.** Documented as infeasible (see
  Context) rather than left as an open TODO that implies it is merely unscheduled. If a userspace egress
  proxy is ever built, it would be a new ADR, not a revision of this one.
- **A future `/config` UI toggle** for "allow network this session" (surfacing `KIRI_SANDBOX_NETWORK` as
  a live setting instead of an env var) is deferred to issue #54 — not part of this security fix.

## Consequences

- `cargo build` / `npm install` no longer get network for free. A user working on a project that needs
  it sets `KIRI_SANDBOX_NETWORK=allow` for the session (or, later, via the planned `/config` toggle).
  This is a deliberate fluidity regression traded for closing the supply-chain residual — stated plainly
  per this project's contract on documented trade-offs.
- Simpler code: `RunCommand` drops a field, a method, and its confirmation branch; `Settings` drops a
  field; two config-loading functions and a defaults list are deleted. Fewer moving parts to audit.
- The macOS Seatbelt profile's `(deny network*)` (already existing since ADR 0009) now uniformly applies
  whenever the session stance is `Deny` — previously it applied except for allow-listed commands.
- Regression tests updated: `network_widens_only_for_a_clean_allowlisted_leading_command` and
  `confirmation_discloses_network_grant_for_allowlisted_command` (which tested the removed mechanism) are
  replaced by `network_is_never_widened_by_command_name`, locking that no command — allow-listed or not —
  ever claims a network grant in the confirmation prompt.
- Closes issue #5 (`security-debt`). Opens issue #54 for the `/config` network toggle (not
  security-debt — a UX convenience, the underlying control already exists via env var).
