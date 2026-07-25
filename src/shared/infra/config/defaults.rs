use std::time::Duration;

/// Wall-clock budget for one turn's tool loop before pausing to ask whether to keep going. The loop has
/// no iteration cap, so this is a guard against an unattended runaway.
pub(super) const TOOL_CHECKPOINT: Duration = Duration::from_secs(30 * 60);

/// The other runaway guard: bounds an auto-mode turn even when every call is fast enough that
/// [`TOOL_CHECKPOINT`] never trips.
pub(super) const MAX_TOOL_CALLS_PER_CHECKPOINT: usize = 100;

/// `read` resets on each chunk (streaming-safe) and is generous because it also bounds the wait for the
/// FIRST chunk — a reasoning model takes a while to emit its first token. Override via `[http]` or
/// `KIRI_HTTP_*_TIMEOUT_MS`.
pub(super) const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// The default provider id and its NVIDIA OpenAI-compatible endpoint, used to seed a first-run config
/// (and the no-regression target). See docs/decisions/0001-openai-compatible-provider.md.
pub(super) const DEFAULT_PROVIDER_ID: &str = "nvidia";

/// Toolchain cache/config directories a build legitimately writes to, allowed for writing under
/// confinement by default so the first `cargo build` / `npm install` works with no extra setup. The
/// list tracks the toolchains the command policy admits (ADR 0030): a program plan mode runs must be
/// able to reach its own cache, or confinement turns an approved command into a confusing failure.
pub(super) const DEFAULT_RW_DIRS: &[&str] = &[
    "~/.cargo",
    "~/.rustup",
    "~/.npm",
    "~/.cache",
    "~/.gradle",
    "~/.m2",
    "~/go",
    "~/.bun",
    "~/.deno",
    "~/.pnpm-store",
    "~/.local/share/uv",
];
