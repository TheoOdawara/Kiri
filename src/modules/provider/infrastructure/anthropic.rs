//! The Anthropic Messages API adapter (API key). Translates a domain `TurnRequest` into the Messages
//! API wire shape — top-level `system`, alternating user/assistant content blocks, `tool_use`/
//! `tool_result`, and OpenAI→Anthropic tool-schema translation — streams the response, and assembles
//! the turn. Authenticates with `x-api-key`; subscription OAuth is intentionally unsupported (see the
//! provider-auth ADR).
pub mod message_dto;
pub mod provider;
pub mod sse;
pub mod wire;
