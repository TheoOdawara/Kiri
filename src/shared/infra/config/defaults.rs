use std::time::Duration;

/// Wall-clock budget for a single user turn's tool loop before pausing to ask the user whether to keep
/// going. There is no fixed iteration cap — the loop runs until the model stops requesting tools — so
/// this time checkpoint is the only guard against an unattended runaway.
pub(super) const TOOL_CHECKPOINT: Duration = Duration::from_secs(30 * 60);

/// Maximum tool calls in a single user turn before the runaway checkpoint fires (alongside the
/// wall-clock budget). Bounds an unattended (auto-mode) runaway to a finite number of actions
/// between check-ins, even when each call is fast enough that the time budget never trips.
pub(super) const MAX_TOOL_CALLS_PER_CHECKPOINT: usize = 100;

/// HTTP client timeouts for the provider. `connect` caps establishing the TCP/TLS connection; `read`
/// caps idle time waiting for the next chunk (response headers or an SSE chunk) — streaming-safe, since
/// it resets on each received chunk, so a legitimately long but active stream is never killed. A hung
/// provider thus fails fast with a clear error instead of hanging forever. `read` is generous (5 min)
/// because it also bounds the wait for the FIRST chunk: a reasoning model can take a while to emit its
/// first token. Overridable via `[http]` in config or `KIRI_HTTP_*_TIMEOUT_MS`.
pub(super) const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// The default provider id and its NVIDIA OpenAI-compatible endpoint, used to seed a first-run config
/// (and the no-regression target). See docs/decisions/0001-openai-compatible-provider.md.
pub(super) const DEFAULT_PROVIDER_ID: &str = "nvidia";

/// Leading programs `run_command` may invoke in plan mode — an **allow-list** of safe
/// inspection/build/test binaries, replacing the former best-effort denylist (a denylist let any
/// unlisted command through and was trivially bypassable; an allow-list defaults to *deny*). Matched
/// against the command's leading program only, and a command that chains a second program is rejected
/// outright (see `run_command::plan_check`), so `cargo test && rm -rf x` never qualifies. Build/test
/// tools (`cargo`, `npm`, …) are included so plan-mode investigation stays fluid; a mutating
/// subcommand of an allowed binary (`git commit`, `cargo install`) still hits the per-call confirmation
/// gate before it runs. Override via `KIRI_PLAN_ALLOW` (newline-separated regexes, replaces this default).
pub(super) const DEFAULT_PLAN_ALLOW: &[&str] = &[
    // Pure inspection.
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
    // Version control (read use; mutating subcommands still hit the confirmation gate).
    r"\bgit\b",
    // Rust toolchain.
    r"\bcargo\b",
    r"\brustc\b",
    r"\brustup\b",
    // JS toolchain.
    r"\bnode\b",
    r"\bnpm\b",
    r"\bnpx\b",
    r"\bpnpm\b",
    r"\byarn\b",
    r"\bdeno\b",
    r"\bbun\b",
    // Python toolchain.
    r"\bpython3?\b",
    r"\bpip3?\b",
    r"\buv\b",
    // Other build runners.
    r"\bmake\b",
    r"\bgo\b",
];

/// Shell commands allowed to reach the network under OS confinement: dev / package-manager tools, so
/// builds and dependency installs stay fluid while arbitrary outbound calls are denied by default. The
/// grant is disclosed in the `run_command` confirmation (informed per-call consent, not silent).
/// Residual: an allow-listed tool's build script still runs with network and could exfiltrate — the real
/// fix is per-host egress filtering, deferred to the cross-OS sandbox work (see `RunCommand::network_for`).
/// Override via `KIRI_SANDBOX_NET_ALLOW_CMDS` (newline-separated regexes, replaces this default).
pub(super) const DEFAULT_NET_ALLOW: &[&str] = &[
    r"\bcargo\b",
    r"\brustup\b",
    r"\bnpm\b",
    r"\bnpx\b",
    r"\bpnpm\b",
    r"\byarn\b",
    r"\bbun\b",
    r"\bpip3?\b",
    r"\bpoetry\b",
    r"\buv\b",
    r"\bgo\b",
    r"\bgit\b",
    r"\bmake\b",
    r"\bmvn\b",
    r"\bgradle\b",
    r"\bbundle\b",
    r"\bgem\b",
    r"\bcomposer\b",
    r"\bdeno\b",
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
