//! `agent` declared only `application` until ADR 0029: its domain types (the conversation) live in
//! `shared/kernel` to break the agent<->provider cycle, and its UI-facing adapters live in `provider`/
//! `tui`. `infrastructure` holds the one exception — the `task` tool dispatches a nested `AgentLoop`, so
//! it needs `agent::application` itself (a plain `tools/infrastructure` fs tool cannot reach `AgentLoop`/
//! `CompletionProvider` without inverting the `tools` -> `agent` dependency direction).

pub mod application;
pub mod infrastructure;
