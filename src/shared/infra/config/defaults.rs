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

/// Patterns blocked in plan mode — commands that mutate the project or escalate privilege.
/// The shell can bypass these (eval, base64, ANSI-C quoting), so this is best-effort; the
/// real fix is OS-level sandboxing (tracked as security-debt in ADR 0002). Override via
/// `KIRI_PLAN_BLACKLIST` (newline-separated, `#` comments, replaces this default).
pub(super) const DEFAULT_PLAN_BLACKLIST: &[&str] = &[
    r"\brm\b",
    r"\bdel\b",
    r"\brmdir\b",
    r"\brd\b",
    r"\bunlink\b",
    r"\btee\b",
    r"\bdd\b",
    r"\bmv\b",
    r"\bmove\b",
    r"\brename\b",
    r"\bcp\b",
    r"\bcopy\b",
    r"\bformat\b",
    r"\bmkfs\b",
    r"\bdiskpart\b",
    r"\bsudo\b",
    r"\bsu\b",
    r"\brunas\b",
    r"git\s+(commit|push|reset|clean|checkout|merge|rebase)",
    r"(npm|pip|cargo|gem|go)\s+install",
    r"\bkill\b",
    r"\bkillall\b",
    r"\btaskkill\b",
];

/// Shell commands allowed to reach the network under OS confinement: dev / package-manager tools, so
/// builds and dependency installs stay fluid while arbitrary outbound calls are denied by default.
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
