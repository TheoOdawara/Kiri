# 0016. First-class keyless local providers

- Status: accepted
- Date: 2026-08-09

## Context

Kiri-V2 must support local models without turning API-key entry into a
prerequisite for using the product. Ollama and LM Studio both expose local
OpenAI-compatible endpoints, while LM Studio can optionally require an API
token. A shared adapter can cover the common protocol without treating a
local provider as a remote credentialed service.

## Decision

Ollama and LM Studio are first-class local providers in Kiri-V2 v1.

- Kiri provides built-in local provider profiles for both products.
- Both profiles use the shared OpenAI-compatible provider adapter for the
  common `/v1` API surface, including model discovery and streaming chat.
- The default Ollama base URL is `http://localhost:11434/v1`.
- The default LM Studio base URL is `http://localhost:1234/v1`.
- Local profiles are keyless by default: they omit `credential`, do not prompt
  for an API key, and do not create a `CredentialStore` entry.
- If a user enables authentication on a local server, the profile may provide
  an opaque credential reference and use the general credential backend. Kiri
  never invents or persists a fake compatibility key.
- Local providers are visible during onboarding and provider selection without
  requiring a remote-provider setup step.
- Local status, model discovery, and capability differences remain explicit
  provider operations; Kiri does not assume that every local model supports
  every optional API capability.

Local means that the configured endpoint is normally loopback; it is not a
policy bypass. Requests still pass through the provider boundary and the
configured network, redaction, timeout, and audit controls.

This decision does not select the remote provider catalog. Remote provider
support remains a separate product-boundary decision.

## Consequences

- A fresh installation can use a locally running model without a secret.
- Ollama and LM Studio share protocol infrastructure while remaining visible
  as distinct user-facing providers.
- Local servers configured to require authentication work with the same
  credential storage decision as remote providers.
- Model and tool support must be discovered or validated rather than assumed
  from the provider name alone.
- The built-in profile catalog must preserve user overrides for base URL and
  model selection without weakening policy validation.

## Alternatives considered

- **Require an API key for every provider:** rejected because keyless local
  inference is a normal supported workflow.
- **Implement separate chat clients for Ollama and LM Studio:** rejected for
  the common OpenAI-compatible surface; provider-specific capabilities can be
  handled behind the shared adapter.
- **Treat local providers as user-authored configuration only:** rejected
  because it makes a primary v1 workflow invisible during onboarding.

## References

- [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)
- [LM Studio OpenAI compatibility](https://lmstudio.ai/docs/developer/openai-compat)
- [LM Studio authentication](https://lmstudio.ai/docs/developer/core/authentication)
