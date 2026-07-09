//! This module has no `domain`: the conversation types live in `shared/kernel` to break the
//! agent<->provider cycle, and the UI-facing adapters live in `provider`/`tui`. `infrastructure` holds one
//! exception (ADR 0029) — the `task` tool needs `agent::application` to dispatch a nested `AgentLoop`, and
//! a plain `tools/infrastructure` tool could not reach it without inverting `tools` -> `agent`.

pub mod application;
pub mod infrastructure;
