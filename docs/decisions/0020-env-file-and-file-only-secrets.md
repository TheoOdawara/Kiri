# ADR 0020 — Trusted `.env` Seeding and File-Only Credential Store

**Status:** Accepted  
**Date:** 2026-07-03  
**Supersedes:** the keyring portion of ADR 0011/0012 (OS keyring as the primary credential store)  
**Relates to:** ADR 0012 (config & secrets storage)

## Context

ADR 0011/0012 stored credentials in the OS keyring (macOS Keychain / Windows Credential Manager / Linux
Secret Service) with a `0600` file as a fallback, and stated a hard **"No `.env`"** invariant — the
untrusted project layer must never redirect a credential or weaken the sandbox; it contributes only
`effort`.

Two problems drove this change:

1. **Keyring friction.** The keyring adapter pulled a heavy, platform-specific dependency tree
   (`security-framework`, `dbus`/`dbus-secret-service`, `zeroize_derive`) and behaved inconsistently
   across headless Linux and CI. The user chose to standardize on a single, portable credential store.
2. **Seeding keys.** Users want a simple, single-file way to keep their API keys and have them imported
   into the harness's own credential store, rather than exporting shell env vars by hand.

A naive `.env` implementation (`dotenvy::dotenv()`, which reads from the **current working directory**)
directly breaks the untrusted-project invariant: a hostile repo the user `cd`s into could ship a `.env`
that injects `KIRI_PATH=/` (widen the fs jail), `KIRI_SANDBOX=off` (disable confinement),
`KIRI_SANDBOX_NET_ALLOW_CMDS` (open egress), `KIRI_PLAN_ALLOW` (widen auto-run), or a `*_API_KEY`
(inject and persist an attacker credential).

## Decision

### 1. Credential store is file-only

`default_secret_store` always returns `FileSecretStore` — the credential is persisted as JSON in
`~/.kiri/credentials.json`, protected `0600` (owner-only) under a `0700` `~/.kiri`. The keyring adapter
and its dependency tree are removed. `AuthMethod::Oauth` remains modeled but non-wired (ADR 0011).

### 2. `.env` is loaded ONLY from the trusted global dir

`main()` calls `config::load_global_env()` before config resolution. It loads `~/.kiri/.env` via
`dotenvy::from_path` — **never** the cwd. `~/.kiri/` is owner-only and user-authored, so it carries the
same trust as `~/.kiri/config.toml`; env it sets (including security-relevant vars and API keys) is
trusted by definition. The cwd — an arbitrary, possibly hostile repo — is never a source of env.

`dotenvy` does not override an already-exported var, so an explicit shell export still wins. Loading is
best-effort: an absent or malformed `.env` leaves the affected key unset and never aborts boot.

The `.env → process env → credentials.json` flow: on first run, a key present in env (from `~/.kiri/.env`
or a real export) is imported and persisted to the credential store for a provider that has none.

### 3. The invariant is guarded, not just documented

`architecture_guards::no_cwd_dotenv_load` walks `src/` and fails the build if the cwd variant
`dotenvy::dotenv()` reappears anywhere. The "project layer is untrusted" invariant thus cannot silently
rot back.

## Consequences

- **Trade-off (accepted):** credentials are now plaintext-at-rest on every platform, a downgrade from
  Keychain/DPAPI encryption. Compensating controls: `0600` file + `0700` dir on Unix. Tracked as
  `security-debt` (plaintext-at-rest; Windows lacks an enforced owner-only ACL — inherited profile DACL
  only). Revisit a macOS-only Keychain adapter if encryption-at-rest becomes a v1 requirement.
- The **"No `.env`"** invariant in `CLAUDE.md` is replaced by **"`.env` only from `~/.kiri/`, never the
  cwd."** The untrusted-project isolation property is preserved: a hostile repo still cannot inject env.
- Windows credential files inherit the user-profile DACL rather than an enforced owner-only ACL; an
  explicit DACL is deferred (`security-debt`) since Windows is a later distro target.
