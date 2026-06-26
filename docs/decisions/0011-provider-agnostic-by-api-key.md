# ADR 0011 — Provider-agnostic harness, by API key only

- Status: Accepted
- Date: 2026-06-26

## Context

ADR 0001 used a single OpenAI-compatible client targeting NVIDIA and foresaw this step: "a future
multi-provider feature externalizes [the base URL] to a config file." We now make the harness
**provider-agnostic** so a user can talk to NVIDIA (default), any OpenAI-compatible / custom endpoint,
**OpenAI (GPT)**, and **Anthropic (Claude)** — selecting and switching among them from inside the TUI.

The original intent included **subscription auth** (Claude Pro/Max and ChatGPT Plus/Pro via OAuth), to
mirror the `opencode` connector ecosystem. We investigated that path before building it.

## Decision

### Multi-vendor behind one port, all by API key

Two wire adapters sit behind the existing `CompletionProvider` port; the `(kind, auth)` → adapter
choice lives in one shared factory (`provider/infrastructure/factory::build_provider`), called by both
the composition root (`app::wire`) and the TUI runtime (live swap):

- **OpenAI chat-completions adapter** — NVIDIA, generic `openai-compatible`, `custom`, and OpenAI proper
  (`api.openai.com`), all with an API key.
- **Anthropic Messages API adapter** (new) — `api.anthropic.com/v1/messages` with `x-api-key`. Translates
  the domain turn into the Messages shape: top-level `system`, strictly-alternating user/assistant
  content blocks, `tool_use`/`tool_result`, and an OpenAI→Anthropic tool-schema translation.

### No subscription OAuth — by deliberate decision, not omission

A dedicated investigation (primary, dated sources; high confidence) found there is **no compliant way**
for a third-party tool to use a user's Claude or ChatGPT **subscription** for inference. The only thing
that works is **impersonating the vendor's official client** (Claude Code / Codex CLI), which:

- is enforced server-side — Anthropic gates `/v1/messages` for subscription OAuth tokens on the Claude
  Code identity (anthropics/claude-code#40515); the ChatGPT Codex backend 403s non-Codex-shaped clients;
- **violates the providers' terms** — Anthropic's 2026-02 Consumer ToS clause restricts Free/Pro/Max
  OAuth tokens to its own products;
- **bans the end user's account** and **exposes the maintainer to legal takedown** — Anthropic forced
  OpenCode (2026-03) and Crush to strip Claude support.

→ **Kiri supports API keys only.** A Claude/GPT user supplies an Anthropic Console / OpenAI Platform API
key (pay-per-token, the sanctioned way to bill against their own account). Subscription OAuth is **not**
built; no client-identity spoofing code exists in the harness. `AuthMethod::Oauth` /
`Credential::Oauth` remain **modeled but non-wired** as an extension point, so a future *sanctioned*
program can slot in without a schema change. An `Oauth` profile fails fast with this rationale.

### In-harness provider management

Three slash commands manage providers live, persisting to the global config (ADR 0012):

- `/models` — switch the active model from the provider's catalog (per-turn field; no rebuild).
- `/effort` — switch the reasoning effort (rebuilds the adapter; effort is captured at construction).
- `/provider` — switch the active provider, or add a new one via a masked-API-key wizard (the key goes
  to the keyring; the profile to the config). The typed key is masked, redacted in `Debug`, zeroized on
  drop, and staged as a `Secret` out of the effect — it never enters an effect, a log, or the transcript.

## Consequences

- **The TUI runtime is a live composition root.** To swap a provider mid-session it rebuilds the adapter
  via the provider factory (`ProviderSwap`), so `tui/infrastructure` depends on
  `provider::infrastructure::factory` — a deliberate, noted deviation from strict
  port-only cross-context dependencies. It is acceptable because the runtime is the front-end's live
  re-composition layer (the same role `app::wire` plays at startup); the alternative — injecting a
  build-capability closure/port at `wire` — is a future refinement, not a correctness issue.
- **Extended thinking on Anthropic is deferred.** The Messages API requires preserving the `thinking`
  block (and its `signature`) across tool-use turns; the domain `Message`/`CompletedTurn` do not model
  provider reasoning traces. Until that lands (a domain change + live verification with a real key), the
  Anthropic adapter sends no `thinking`/`output_config`, keeping tool-use round-trips correct on the
  opt-in-thinking Claude models. `/effort` therefore does not affect Claude yet, and says so visibly.
- Adding a vendor = one adapter behind `CompletionProvider` + a `(kind, auth)` arm in the factory.
- Supersedes ADR 0001's "NVIDIA only / base URL as a `const`": the base URL, model, and provider now come
  from config (ADR 0012); NVIDIA remains the seeded default.
