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

/// Leading programs `run_command` may invoke in plan mode. An allow-list, not a denylist: it defaults to
/// *deny*. Matched against the leading program only, and a chained command is rejected outright (see
/// `run_command::plan_check`), so `cargo test && rm -rf x` never qualifies. Override via
/// `KIRI_PLAN_ALLOW` (newline-separated regexes, replaces this default).
pub(super) const DEFAULT_PLAN_ALLOW: &[&str] = &[
    r"\bls\b",
    r"\bcat\b",
    r"\bhead\b",
    r"\btail\b",
    r"\bwc\b",
    r"\becho\b",
    r"\bpwd\b",
    r"\bwhich\b",
    r"\benv\b",
    r"\bprintenv\b",
    r"\btree\b",
    r"\bstat\b",
    r"\bfile\b",
    r"\bgrep\b",
    r"\brg\b",
    r"\bfind\b",
    r"\bfd\b",
    // Read use; a mutating subcommand (`git commit`, `cargo install`) still hits the confirmation gate.
    r"\bgit\b",
    r"\bcargo\b",
    r"\brustc\b",
    r"\brustup\b",
    r"\bnode\b",
    r"\bnpm\b",
    r"\bnpx\b",
    r"\bpnpm\b",
    r"\byarn\b",
    r"\bdeno\b",
    r"\bbun\b",
    r"\bpython3?\b",
    r"\bpip3?\b",
    r"\buv\b",
    r"\bmake\b",
    r"\bgo\b",
];

/// Toolchain cache/config directories a build legitimately writes to, allowed for writing under
/// confinement by default so the first `cargo build` / `npm install` works with no extra setup.
pub(super) const DEFAULT_RW_DIRS: &[&str] = &[
    "~/.cargo",
    "~/.rustup",
    "~/.npm",
    "~/.cache",
    "~/.gradle",
    "~/.m2",
    "~/go",
];
