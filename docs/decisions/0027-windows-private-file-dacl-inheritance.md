# ADR 0027 — Owner-only files/dirs on Windows: accepted DACL inheritance, not an explicit ACL API

- Status: Accepted
- Date: 2026-07-06
- Relates to: ADR 0020 (env file and file-only secrets — the 0600 credentials file this formalizes the
  Windows side of)

## Context

Audited as issue #36. Every harness-owned private location — the `~/.kiri` dir itself, the credentials
file (`provider/infrastructure/secrets/file_store.rs`), the extensions trust store
(`extensions/infrastructure/trust_store.rs`), and the sync work-tree's exported memory NDJSON
(`sync/infrastructure/memory_ndjson.rs`) — is created/written owner-only on Unix: `0700` dirs
(`ensure_private_dir`), `0600` files, set at `open()` (no post-write chmod window) and re-coerced on every
write in case a stale file was left wider by an interrupted run or an older version. On Windows, `std`
exposes no ACL/DACL manipulation API at all — reaching one would need either `unsafe` Win32 FFI (this
crate's `unsafe_code = "forbid"` lint rules that out) or a new dependency (e.g. `windows-acl`) whose only
job would be a platform this project hasn't shipped for yet (macOS is v1; Windows/Linux are a later port).

Each of the four call sites already independently arrived at the same fallback and documented it inline:
on Windows, the file/dir inherits the ACL of its parent directory — which is itself the user's own
profile directory (`%USERPROFILE%`), whose default Windows ACL grants full control only to the owning
user, `SYSTEM`, and `Administrators` — never "Everyone" or "Authenticated Users." This is a materially
different mechanism than Unix's explicit `0700`/`0600` mode bits, but arrives at an equivalent practical
guarantee for the threat this exists to mitigate: another unprivileged, non-administrator local account
cannot read the file. **Caveat, honestly stated rather than silently assumed:** this guarantee rests on the
*default* local NTFS profile ACL. It is not verified here against a domain-joined machine with Group
Policy overriding profile ACLs, a roaming profile, or a profile redirected to a network share (where the
share's own SMB permissions — not NTFS ACLs — would govern) — none of which can be checked from this
project's current macOS-only dev/CI hosts. Should Windows support actually ship, this is the first thing
to verify on a real Windows host, not an assumption to keep carrying silently.

## Decision

**Accept DACL inheritance as the Windows-side implementation of "owner-only," formalized here rather than
left as four independently-arrived-at inline comments with no single cross-cutting record.** No further
code change to the ACL/mode-bit story: an explicit ACL-setting implementation is deferred indefinitely, not
attempted, since it would require exactly the FFI or dependency surface this project avoids for a platform
not yet targeted.

Every one of the four sites keeps the SAME crash-atomicity guarantee regardless of platform — every
platform branch routes through a temp-sibling-then-rename, never a plain truncate-then-write. This was NOT
already true for `sync/infrastructure/memory_ndjson.rs`'s Unix branch before this ADR's own review: it
opened the export target directly with `truncate`, while its own non-Unix fallback was already atomic via
`write_atomic` — backwards from the parity this ADR claims. Fixed as part of closing this issue (now via a
temp sibling + rename on both platforms, with the pre-existing symlink-refusal guard extended to also cover
the temp sibling, mirroring `fs_work_tree.rs`'s existing pattern for the same reason) rather than shipping
an ADR whose central claim didn't hold for one of the four sites it names.

## Consequences

- If Windows support is ever built out, the right escalation is an explicit ACL write (via a vetted,
  minimal dependency, or Win32 FFI reviewed as its own security-relevant change) — not attempted now,
  and not silently implied as already solved. The domain-policy/roaming-profile caveat above should be the
  first thing verified on a real Windows host.
- `sync/infrastructure/memory_ndjson.rs`'s Unix export path is now genuinely crash-atomic, matching its own
  non-Unix fallback and this ADR's claim. Locked by
  `export_is_crash_atomic_and_leaves_no_temp_sibling` and `export_refuses_a_symlinked_temp_sibling`
  (`memory_ndjson.rs`).
- Locked by `owner_only_writers_have_both_platform_branches` (`architecture_guards.rs`): scans the four
  wrapper files (`shared/infra/config/writers.rs`, `provider/infrastructure/secrets/file_store.rs`,
  `extensions/infrastructure/trust_store.rs`, `sync/infrastructure/memory_ndjson.rs`) and asserts each
  contains BOTH a `#[cfg(unix)]` and a `#[cfg(not(unix))]` branch for its owner-only writer — so a future
  edit that silently drops the non-Unix fallback (invisible on this project's macOS dev/CI hosts, since
  only the Unix branch needs to compile there) fails a test instead of shipping unnoticed. `shared/infra/fs.rs`
  (the Unix-only `write_atomic_owner_only` building block the wrappers call directly, with no
  `#[cfg(not(unix))]` sibling of its own by design) is checked separately, for presence only.
- Closes #36.
