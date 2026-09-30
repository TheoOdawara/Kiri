# ADR 0016 — Keyless local providers (no-auth) and forward-compatible auth

- Status: Accepted
- Date: 2026-06-27

## Context

ADR 0011 made the harness provider-agnostic **by API key only**, modelling `AuthMethod` as
`{ ApiKey, Oauth }` and `Credential` as `{ ApiKey, Oauth }`. That left no way to express *no
authentication*: the factory always demanded a key, the OpenAI-compatible adapter always sent
`Authorization: Bearer …`, the add-provider wizard trapped on an empty key, and a live `/provider`
switch errored with `no credential for provider`.

But the most common local-LLM runtimes — **Ollama and LM Studio** — expose an OpenAI-compatible endpoint
that needs **no API key by default**. A user configuring `openai-compatible` against LM Studio could not
get past the wizard, and a hand-edited config booted into onboarding. Keyless is the *normal* path for
these providers, not an edge case.

## Decision

### Key presence decides the auth method, within a per-kind floor

We add `AuthMethod::None` and `Credential::None`. The rule: **the presence of a key at save time decides
the auth method, per provider** — not the kind. The kind only sets a *floor*:

- Vendor kinds (NVIDIA / OpenAI / Anthropic) **require** a key — `ProviderKind::requires_api_key()` is the
  single source of truth, used by both the wizard (a blank key keeps it on the step) and the factory (a
  keyless vendor profile fails fast).
- `openai-compatible` / `custom` may be **keyless or keyed**: a blank key → `auth = "none"`; a typed key →
  `auth = "api-key"`. This keeps remote OpenAI-compatible services that *do* require a key (OpenRouter,
  Together, Groq, …) working through the same path.

A keyless provider stores **nothing** in the keyring; the composition root and the live-swap runtime
synthesise `Credential::None`, and the OpenAI-compatible chat/embeddings adapters **omit** the
`Authorization` header entirely (never an empty `Bearer `, which some local servers reject).

### Forward-compatible `auth` — never abort the boot

`AuthMethod` gains `Unknown(String)` with hand-written serde: an `auth` value this build does not
recognise (e.g. written by a newer Kiri) deserialises **losslessly** instead of failing. The factory
leaves such a provider inert (a clear error), and `app::wire` tolerates a `build_provider` failure for the
active provider by routing to onboarding — so a misconfigured or forward-version config **never aborts the
boot**, preserving the ADR 0011 "never abort" invariant.

### Named keyless provider ids

The wizard adds a `ProviderId` step for keyless-capable kinds, so several compatible endpoints (a local
LM Studio and a remote OpenRouter) can coexist instead of colliding on a per-kind id. The typed id is
sanitised to a stable `[a-z0-9_-]` token; vendor kinds keep their canonical id.

### Sync trust gate covers auth

The portable-profile sync trust gate (ADR 0015) now reads `auth`: a synced config that downgrades an
existing provider from a keyed method to `none` (silently disabling its credential — and, for a vendor
endpoint, bricking the next boot) is flagged as risky and requires `--force`.

## Consequences

- LM Studio / Ollama work keyless end-to-end: save with an empty key, switch, and stream with no
  `Authorization` header. NVIDIA and keyed OpenAI-compatible providers are unaffected.
- **Forward-compat residual (accepted):** a Kiri binary *older* than this change cannot read `auth =
  "none"` (unknown variant). This is inherent — old code cannot be taught a new value. It is benign here:
  `sync pull` on the old machine rejects the config via `validate_config_str` and keeps its working one
  (no brick), and a same-machine downgrade is self-inflicted and rare.
- `AuthMethod` loses `Copy` (the `Unknown(String)` payload); the only call site affected matches it by
  reference. No persisted config or keyring entry changes shape — the variant is purely additive.

Amends ADRs 0011 (provider-agnostic by API key) and 0012 (config and secrets storage); composes with
ADR 0015 (portable-profile sync).
