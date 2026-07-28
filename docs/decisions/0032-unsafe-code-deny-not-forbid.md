# ADR 0032 — `unsafe_code` relaxed from `forbid` to `deny`, for exactly one module

- Status: Accepted
- Date: 2026-07-26
- Amends: the crate-wide `[lints.rust] unsafe_code = "forbid"` in `Cargo.toml`, which ADR 0027 cited as
  an argument for its own posture.

## Context

The crate has carried `unsafe_code = "forbid"` since early on, and it has been load-bearing in review:
ADR 0027 leaned on it when arguing that the harness cannot hand-roll a Windows ACL primitive. `forbid`
is stronger than `deny` in one specific way — it cannot be overridden by an inner `#[allow]`, so no file
can opt back in.

ADR 0031's write confinement needs `CreateRestrictedToken`, `CreateProcessAsUserW`,
`SetTokenInformation`, `GetTokenInformation`, `SetEntriesInAclW`, and `SetNamedSecurityInfoW`. Every one
is an `unsafe extern "system"` call. There is no safe-Rust path: `std::os::windows::process::CommandExt`
exposes creation flags and raw args but no token, and `process-wrap` — already a dependency for
tree-kill — exposes no token either. macOS and Linux need none of this because Apple and the bubblewrap
project ship a launcher binary; Windows ships nothing equivalent, so the harness must make the calls.

## Decision

Relax the crate lint to `deny`, and confine the opt-in to a single file:

```toml
[lints.rust]
unsafe_code = "deny"
```

```rust
// src/modules/tools/infrastructure/confine/windows/restricted.rs
#![allow(unsafe_code)]
```

`deny` still fails the build for `unsafe` written anywhere else — the gate runs
`clippy --all-targets -- -D warnings`, so an accidental `unsafe` in any other file is a hard error, not
a warning. What changes is only that one file may say so out loud, in one line, at the top, where a
reviewer sees it before any code.

**Scope discipline, so this stays one file:**

- The module holds only the Win32 sequence. The adapter that decides *when* to confine, builds the
  command, and computes the writable roots (`confine/windows.rs`) contains no `unsafe` and is covered by
  ordinary tests.
- Every `unsafe` block carries a `SAFETY:` comment naming the invariant it relies on — buffer sized to
  `SECURITY_MAX_SID_SIZE`, pointer outliving the call, handle closed on every path.
- Pointers handed to the OS come from locals that outlive the call; every handle opened is closed on
  both the success and the error path.

## Consequences

- The crate-wide guarantee is now "no `unsafe` outside one named, reviewed file" rather than "no
  `unsafe`, structurally". That is a real reduction in what the lint proves, and it is why this needs a
  decision record rather than a commit message.
- ADR 0027's argument still holds in substance: the harness does not hand-roll ACL manipulation
  *casually*. It does so in one module, for one documented mechanism, with the alternative being no
  Windows confinement at all.
- Should the Windows confinement ever be removed, the lint returns to `forbid` in the same change. The
  relaxation exists for this module and has no other justification.
