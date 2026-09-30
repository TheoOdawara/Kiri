# ADR 0001 — OpenAI-compatible chat client targeting NVIDIA

- Status: Accepted
- Date: 2026-06-18

## Context

The bootstrap architecture (`CLAUDE.md`) named `src/services/ollama.rs` as the owner of an
Ollama-specific HTTP client targeting `http://<host>:11434/v1/chat/completions`. We are dropping the
local Ollama target and using **NVIDIA's hosted inference** (`https://integrate.api.nvidia.com/v1`) as
the provider, which exposes the **same** OpenAI-compatible chat-completions protocol. The differences
between providers are only the base URL, the bearer API key, and the model name — values, not protocol
shape — so the client is written against the protocol, not the vendor.

Keeping an `ollama`-named service would be misleading now that it targets NVIDIA.

## Decision

Use a single OpenAI-compatible chat service, `src/services/chat.rs`, that takes the base URL, API key,
and model as inputs (no hardcoding inside the service).

- **Provider:** NVIDIA only, for now.
- **Base URL:** hardcoded at the composition root as `const BASE_URL` in `main.rs`. A future
  multi-provider feature ("connect to any provider") will externalize it to a separate config file;
  because the service already receives `base_url` as a parameter, that change is config-only.
- **API key & model:** read from the environment (`.env` loaded via `dotenvy` at startup), both
  **required** (fail-fast with a clear error if absent):
  - `NVIDIA_API_KEY` — bearer token, always sent as `Authorization: Bearer`.
  - `NVIDIA_MODEL` — model id (any model offered by NVIDIA).
- The API key is **not** a CLI flag, to avoid leaking it in `--help` output or the process argument
  list — it is read only from the environment.

## Consequences

- `src/services/ollama.rs` is renamed to `src/services/chat.rs`; `CLAUDE.md` is updated accordingly.
- The architecture's one-way dependency rule (`main → services → models`) is unchanged.
- Multi-provider support (provider config in a separate file, optional/no-auth providers) is a later
  feature, intentionally not built now (YAGNI). When it lands it replaces the `BASE_URL` const with a
  config read; the service signature already supports it.
