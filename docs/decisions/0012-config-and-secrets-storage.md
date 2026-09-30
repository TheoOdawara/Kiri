# ADR 0012 — Configuration & secrets: layered TOML + OS keyring, no `.env`

- Status: Accepted
- Date: 2026-06-26

## Context

ADR 0001 read the provider key and model from a `.env` file (`dotenvy`), both required. Going
provider-agnostic (ADR 0011) means the harness manages a **catalog** of providers, the active selection,
per-provider model lists, the reasoning effort, and one **secret** per provider — too much for `.env`,
and secrets do not belong in a plaintext dotfile. The harness now owns its configuration and secrets.

A coding agent operates on **untrusted repositories**. Any per-project config it reads is therefore an
attack surface: a malicious repo must not be able to redirect a stored credential to an attacker endpoint
or weaken the command sandbox.

## Decision

### Layered TOML config

- **Global** `~/.kiri/config.toml` (trusted) ← **project** `<workspace>/.kiri/config.toml` (untrusted).
- `~/.kiri` is the harness's home for config, the credentials fallback file, and the shared-memory DB,
  created `0700` on Unix. The literal `~/.kiri` path is intentional (not a platform config dir).

### Workspace-trust model (security-critical)

`resolve_layers` honors **only the `effort` preference** from the project (untrusted) layer. Provider
definitions, the active selection, and the `sandbox`/`http`/`behavior`/`paths` policy come from the
**trusted global layer only**. Rationale: a repo shipping a `.kiri/config.toml` must not be able to

- reuse a provider id with a different `base_url` to redirect a stored credential to its own endpoint
  (credential exfiltration), or
- ship `[sandbox] mode = "off"` to weaken command confinement.

A regression test locks this. Broader, trust-gated per-project config (beyond `effort`) is deliberate
future work.

### Secrets in the OS keyring, never in TOML

One `Credential` per provider id is stored via the `SecretStore` port: an **OS keyring** adapter
(macOS Keychain / Windows Credential Manager / Linux Secret Service, no per-OS branches) with a **`0600`
file fallback** (`~/.kiri/credentials.json`) for headless/CI hosts with no keyring. The backend is probed
once at startup to avoid a split-brain store. Secrets are modeled by a `Secret` type (zeroized on drop,
redacted in `Debug`) and are **never** written to the TOML config or a log.

### Migration from `.env`

`.env`/`dotenvy` are removed. A first run with no global config seeds the default NVIDIA provider and
writes a starter `~/.kiri/config.toml`. For an API-key provider, a legacy/CI env var
(`NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / generic `KIRI_<ID>_API_KEY`) is imported
**once** into the credential store, then no longer needed. The same env resolution is the fallback when
switching to a provider whose key is not yet stored.

### Live writes

`/models`/`/effort`/`/provider` persist via a **read-modify-write of the global config that preserves
every section** (unlike the first-run starter writer). Only the trusted global file is written.

## Consequences

- `dotenvy` and the `secrecy` crate are dropped; `toml`, `keyring`, and `zeroize` are added; config moves
  from required env vars to TOML-with-env-and-default resolution for harness knobs.
- TOML **comments** in a hand-edited config are dropped on a harness rewrite (values are preserved);
  comment-preserving edits (`toml_edit`) are a possible future refinement.
- Secrets I/O and the config files are **harness-owned storage** — the same carve-out from the
  filesystem-sandbox invariant as the `memory` context (ADR 0010), never used for agent-supplied paths.
