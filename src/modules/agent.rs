//! `agent` declares only `application` by design: its domain types (the conversation) live in
//! `shared/kernel` to break the agent<->provider cycle, and its adapters live in `provider`/`tui`, so the
//! single-layer module is intentional, not an omission.

pub mod application;
