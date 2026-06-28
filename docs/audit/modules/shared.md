# Audit — Shared kernel + infra

> Scope: `src/shared.rs`, `src/shared/infra.rs`, `src/shared/infra/config.rs`, `src/shared/kernel.rs` and all of `src/shared/kernel/` (`provider.rs`, `conversation.rs`, `message.rs`, `error.rs`, `tool_call.rs`, `stream_event.rs`, `completed_turn.rs`, `role.rs`, `approval_mode.rs`)
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The kernel is in genuinely good shape: pure data/rules, no I/O, `Secret` is correctly zeroized and `Debug`-redacted, the `AuthMethod`/`ProviderKind` forward-compat serde is well-tested, and the untrusted-project-layer security boundary (`resolve_layers`) is real and regression-tested. The headline issues live in `config.rs`: it is the one shared file that depends *into* `modules/tools` (including a tools **infrastructure** constructor), inverting the intended dependency direction; it leaks `anyhow` into the live TUI runtime through its `persist_*` writers; and at ~1054 lines it bundles the CLI, the system prompt, the sandbox-policy defaults, the TOML model, and the `Settings` resolver into a single god-file. Secondary themes: the hardcoded system prompt duplicates tool inventory/limits/sensitive-pattern values that authoritatively live elsewhere (drift risk), serde discipline is applied inconsistently inside the kernel (`ToolCall` carries OpenAI-wire shaping while `Message`/`Role` deliberately do not), and several security-relevant config-resolution branches (sandbox mode/network, blacklist-fallback, dir hardening) are untested or swallow their failures.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 6 | 8 |

## Findings

### [SHARED-01] `shared/infra/config` depends into `modules/tools` (application **and** infrastructure)
- **Severity:** High
- **Category:** architecture
- **Location:** `src/shared/infra/config.rs:11`, `src/shared/infra/config.rs:12`
- **Problem:** `shared` is meant to be the leaf that every module depends *on*, never the reverse. `config.rs` is the only file under `src/shared` that does `use crate::modules::...`, and it reaches into two tools layers at once: `tools::application::command_sandbox::NetworkPolicy` and, worse, `tools::infrastructure::sensitive::{SensitiveMatcher, load_sensitive_matcher}`. `load_sensitive_matcher()` is an infrastructure constructor (env + regex wiring), so `shared/infra` is invoking another module's adapter wiring — an inverted dependency. This couples the kernel-adjacent config aggregate to a concrete module's infrastructure and undermines the "depend inward / shared is the bottom" rule.
- **Evidence:**
```rust
use crate::modules::tools::application::command_sandbox::NetworkPolicy;
use crate::modules::tools::infrastructure::sensitive::{SensitiveMatcher, load_sensitive_matcher};
```
- **Recommendation:** Treat `Settings` assembly as a composition-root concern: move the parts that need tools types (the `SensitiveMatcher` build, `NetworkPolicy`) up to `app.rs`/`wire`, or inject the already-built `SensitiveMatcher`/`NetworkPolicy` into a pure `Settings` resolver. At minimum, depend only on a tools *port/domain* type, never on `tools::infrastructure`.

### [SHARED-02] Config writers return `anyhow::Result` and are called from the live TUI runtime
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/shared/infra/config.rs:378`, `src/shared/infra/config.rs:390`, `src/shared/infra/config.rs:395`, `src/shared/infra/config.rs:404`; call sites `src/modules/tui/infrastructure/runtime.rs:977`, `src/modules/tui/infrastructure/runtime.rs:1015`, `src/modules/tui/infrastructure/runtime.rs:1053`, `src/modules/tui/infrastructure/runtime.rs:1127`
- **Problem:** The contract is "ports return `AgentError`; `anyhow` only at the binary edge." `persist_active_model` / `persist_effort` / `persist_active_provider` / `upsert_provider` all return `anyhow::Result<()>` and are invoked deep inside the TUI runtime's live `/models`, `/effort`, `/provider` effect handlers — not at the binary edge. So `anyhow::Error` flows back into runtime-reachable code, diverging from the stated invariant (boot-time `Settings::resolve` returning `anyhow` is defensible as edge; these live writers are not).
- **Evidence:**
```rust
pub fn persist_active_model(config_path: &Path, provider_id: &str, model: &str) -> Result<()> {
```
- **Recommendation:** Have the config writers return `Result<(), AgentError>` (e.g. a new `AgentError::Config` or reuse `AgentError::Io`/`Secret`), keeping `anyhow` confined to `main`/`app`. The runtime already pattern-matches `Err(error)` and surfaces a notice, so the change is local.

### [SHARED-03] `config.rs` is an oversized, multi-responsibility god-file
- **Severity:** Medium
- **Category:** file-size
- **Location:** `src/shared/infra/config.rs:1` (entire 1054-line file)
- **Problem:** One file holds at least six distinct responsibilities: the 100-line `SYSTEM_PROMPT` literal (lines 20-119), the sandbox/plan/net policy default constant lists (lines 124-209), the `RawConfig` TOML model + `is_empty` impls (lines 218-289), the read/validate/write/persist functions (lines 323-408, 778-826), the env/bool/duration/path parsing helpers (lines 438-552), the clap `Cli`/`CliCommand`/`SyncAction` definitions (lines 554-600), and the `Settings` struct + `resolve` (lines 605-776). That is well beyond a single responsibility and makes the file hard to navigate and review.
- **Evidence:**
```rust
const SYSTEM_PROMPT: &str = concat!( /* ~100 lines of prose */ );
// ... later in the same file ...
#[derive(Parser)]
pub struct Cli { /* clap CLI */ }
// ... later still ...
pub struct Settings { /* 25+ resolved fields */ }
```
- **Recommendation:** Split under `src/shared/infra/config/`: `system_prompt.rs` (the const), `cli.rs` (clap types), `raw_config.rs` (`RawConfig` + `Raw*` + read/validate), `writers.rs` (`update_global_config` + `persist_*` + `upsert_provider` + `write_starter_config`), `defaults.rs` (blacklist / net-allow / rw-dir constants), and `settings.rs` (`Settings` + `resolve` + `resolve_*` helpers).

### [SHARED-04] `SYSTEM_PROMPT` hardcodes tool inventory, limits, and sensitive patterns that authoritatively live in code
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/shared/infra/config.rs:44`, `src/shared/infra/config.rs:61`, `src/shared/infra/config.rs:92`, `src/shared/infra/config.rs:107`
- **Problem:** The prompt prose restates values that are owned elsewhere and will silently drift: it asserts "ten file tools" (the count duplicates `default_fs_tools`), "30s timeout enforced; output truncated at 64 KiB" (duplicates the run_command tool's constants), "Every ~30 minutes" (duplicates `TOOL_CHECKPOINT` = `30 * 60`), and an explicit list of sensitive filename patterns `.env*, id_rsa, ... known_hosts` (duplicates the `SensitiveMatcher` defaults in `tools/infrastructure/sensitive`). Adding/removing a tool or changing a limit leaves the prompt lying to the model with no compile-time signal.
- **Evidence:**
```rust
"You have ten file tools, grouped by effect on the filesystem, plus one plan-mode control tool.\n",
// ...
"Sensitive names: .env*, id_rsa, id_dsa, id_ecdsa, id_ed25519, *.pem, *.key, ",
```
- **Recommendation:** Either generate the tool list / limits / sensitive-pattern summary from the registry and `SensitiveMatcher` defaults at startup, or drop the precise counts/values from the prose and reference the live source ("the tools advertised this turn", "the configured sensitive patterns") so there is one source of truth.

### [SHARED-05] Serde discipline is applied inconsistently inside the kernel
- **Severity:** Medium
- **Category:** inconsistency
- **Location:** `src/shared/kernel/tool_call.rs:6`, `src/shared/kernel/tool_call.rs:9`, `src/shared/kernel/tool_call.rs:21`; contrast `src/shared/kernel/message.rs:4`, `src/shared/kernel/role.rs:1`
- **Problem:** `message.rs` and `role.rs` explicitly keep the domain serde-free ("no wire/serde concern — the provider maps it … via a DTO"), yet the sibling kernel type `ToolCall`/`FunctionCall` carries `#[derive(Serialize, Deserialize)]`, an OpenAI-wire `#[serde(rename = "type")]`, and a `default_function_type()` returning the wire literal `"function"`. So the "domain is serde-free, the DTO owns the wire" rule is honored for messages/roles but violated for tool calls in the same layer, and OpenAI-specific wire shaping leaks into the kernel.
- **Evidence:**
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_type")]
    pub kind: String,
    pub function: FunctionCall,
}
```
- **Recommendation:** Decide one rule for the kernel and apply it uniformly. If `ToolCall` needs serialization only for session persistence, give it a neutral/persistence schema and push the OpenAI `type`/`function` wire details into the provider DTO (as `Message` already does); if serde is acceptable in the kernel, document why and reconsider the serde-free comments on `message.rs`/`role.rs`.

### [SHARED-06] Security-relevant config-resolution branches are untested
- **Severity:** Medium
- **Category:** error-handling
- **Location:** `src/shared/infra/config.rs:471` (`resolve_sandbox_mode`), `src/shared/infra/config.rs:484` (`resolve_sandbox_network`), `src/shared/infra/config.rs:525` (`compile_patterns` empty-override fallback, lines 534-544), `src/shared/infra/config.rs:446` (`resolve_timeout`), `src/shared/infra/config.rs:501` (`expand_home`)
- **Problem:** The test module (lines 828-1054) covers parsing, layer security, and the writers, but several security-relevant pure resolvers have no test: that `KIRI_SANDBOX=off`/`require` map correctly, that `KIRI_SANDBOX_NETWORK` defaults to `Deny`, and — most importantly — that `compile_patterns` falls back to the *default* safety list (rather than silently disabling it) when an override strips to zero usable patterns. The contract requires error/security paths to be tested like behavior; these branches gate command confinement and the plan-mode blacklist.
- **Evidence:**
```rust
// A non-empty override that filters to zero patterns (e.g. all comments) would silently
// disable a safety list ... Fall back to the defaults rather than accept "silently empty".
if filtered.is_empty() { /* ... */ defaults.to_vec() }
```
- **Recommendation:** Add unit tests for `resolve_sandbox_mode`/`resolve_sandbox_network` (off/require/allow/deny/unknown), the `compile_patterns` empty-after-strip fallback, `resolve_timeout` precedence (config > env > default), and `expand_home` (`~`, `~/x`, and HOME-unset).

### [SHARED-07] Stale / self-contradictory doc comment on `Settings::resolve`
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/shared/infra/config.rs:663`
- **Problem:** The doc block has two redundant paragraphs that contradict each other: the first says "Parse the CLI, load the layered TOML config …", but the function does **not** parse the CLI — it receives already-parsed `cli_path`/`cli_prompt`, as the second paragraph correctly states ("Resolve settings from the already-parsed CLI path/prompt. `main` parses the CLI …"). The first paragraph is stale from when `resolve` owned CLI parsing.
- **Evidence:**
```rust
/// Parse the CLI, load the layered TOML config (`~/.kiri` ← `<workspace>/.kiri`), and resolve the
/// ...
/// Resolve settings from the already-parsed CLI path/prompt. `main` parses the CLI (so it can
/// dispatch the headless `kiri sync` route before reaching the TUI) and hands the values here.
pub fn resolve(cli_path: Option<PathBuf>, cli_prompt: Option<String>) -> Result<Self> {
```
- **Recommendation:** Collapse to one accurate paragraph that says `resolve` takes already-parsed CLI values and loads/resolves the layered config; drop the "Parse the CLI" lead.

### [SHARED-08] `write_starter_config` takes `&PathBuf` while every sibling takes `&Path`
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/shared/infra/config.rs:808`
- **Problem:** All other path-taking functions use `&Path` (`read_config_file`, `update_global_config`, `persist_*`), but `write_starter_config(path: &PathBuf, …)` takes `&PathBuf` despite only using `Path` methods (`parent`, passed to `std::fs::write`). Besides the inconsistency this is the textbook `clippy::ptr_arg` shape, which under the project's `clippy --all-targets -- -D warnings` gate is an error.
- **Evidence:**
```rust
fn write_starter_config(path: &PathBuf, providers: &[ProviderProfile], active: &str) -> Result<()> {
```
- **Recommendation:** Change the parameter to `&Path` to match siblings and clear the lint.

### [SHARED-09] `resolve_providers` re-sorts an already-sorted vec
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/shared/infra/config.rs:785`
- **Problem:** `table` is a `BTreeMap<String, ProviderProfile>`, so `into_iter()` already yields entries in ascending key order, and each profile's `id` is set to that very key. The subsequent `providers.sort_by(|a, b| a.id.cmp(&b.id))` therefore re-sorts an already-sorted vector — redundant work that also misleads a reader into thinking input order is arbitrary.
- **Evidence:**
```rust
let mut providers: Vec<ProviderProfile> = table
    .into_iter()
    .map(|(id, mut profile)| { profile.id = id; profile })
    .collect();
providers.sort_by(|a, b| a.id.cmp(&b.id));
```
- **Recommendation:** Drop the `sort_by` (the BTreeMap guarantees order) or add a comment if the explicit sort is kept as defensive intent.

### [SHARED-10] `Secret::Deserialize` uses a redundant, lossy `map_err`
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/shared/kernel/provider.rs:223`; contrast `src/shared/kernel/provider.rs:97`
- **Problem:** `Secret::deserialize` does `String::deserialize(deserializer).map_err(D::Error::custom)`, but `String::deserialize::<D>` already returns `D::Error`; wrapping it in `D::Error::custom` stringifies and re-wraps the original error, losing its structure. The sibling `AuthMethod::deserialize` (line 99) does the idiomatic `String::deserialize(deserializer)?`, so the two diverge for no reason.
- **Evidence:**
```rust
let raw = String::deserialize(deserializer).map_err(D::Error::custom)?;
```
- **Recommendation:** Use `String::deserialize(deserializer)?` to match `AuthMethod` and preserve the deserializer's native error.

### [SHARED-11] Home/path expansion is Unix-only and breaks on Windows drive paths
- **Severity:** Low
- **Category:** architecture
- **Location:** `src/shared/infra/config.rs:501` (`expand_home`), `src/shared/infra/config.rs:515` (`load_extra_paths`)
- **Problem:** `expand_home` keys solely on the `HOME` env var (no Windows `USERPROFILE`), and `load_extra_paths` splits the path list on `':'`, which silently mangles Windows drive-qualified paths like `C:\Users\x`. `ensure_private_dir` already has a `#[cfg(not(unix))]` arm, signalling Windows is in scope, so the home/path handling is inconsistent with that intent. (macOS is the stated v1 target, so this is forward-looking, not a current break.)
- **Evidence:**
```rust
} else if let Some(rest) = path.strip_prefix("~/")
    && let Some(home) = std::env::var_os("HOME")
{ return PathBuf::from(home).join(rest); }
```
- **Recommendation:** When Windows support lands, fall back to `USERPROFILE` (or `dirs`/`home` crate) and split path lists on the platform separator (`;` on Windows) rather than a hardcoded `':'`.

### [SHARED-12] Invalid sandbox env values silently fall back to defaults without warning
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/shared/infra/config.rs:471` (`resolve_sandbox_mode`), `src/shared/infra/config.rs:484` (`resolve_sandbox_network`)
- **Problem:** A typo like `KIRI_SANDBOX=of` or `KIRI_SANDBOX_NETWORK=allwo` hits the `_ =>` arm and silently resolves to the secure default with no feedback — the user believes they disabled/changed confinement but did not. This is inconsistent with `compile_patterns` (lines 538-540), which deliberately warns when an override is unusable. Secure-by-default is good, but the silent no-op on user intent contradicts the project's "no silent no-ops" rule.
- **Evidence:**
```rust
match raw.as_deref() {
    Some("off") => (false, false),
    Some("require") => (true, true),
    _ => (true, false),
}
```
- **Recommendation:** Emit a one-line `eprintln!` warning when the value is non-empty but unrecognized (mirroring `compile_patterns`), then fall back to the safe default.

### [SHARED-13] Inconsistent module-header convention across kernel siblings
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/shared/kernel/provider.rs:1`; contrast `src/shared/kernel/message.rs:1`, `src/shared/kernel/tool_call.rs:1`, `src/shared/kernel/error.rs:1`, `src/shared/kernel/role.rs:1`, `src/shared/kernel/stream_event.rs:1`, `src/shared/kernel/completed_turn.rs:1`
- **Problem:** Only `provider.rs` opens with a `//!` module-level doc explaining the file's role; the other eight kernel files start directly with `use`/`///` and rely on per-item docs. For a cohesive `kernel/` folder this uneven convention makes it harder to scan what each file owns.
- **Evidence:**
```rust
//! Cross-cutting provider primitives: which vendor/protocol a provider speaks, how it authenticates,
//! the reasoning effort, the (non-secret) configured profile, and the secret credential material.
```
- **Recommendation:** Pick one convention for the kernel — either add a one-line `//!` header to each file, or treat the per-item `///` docs as sufficient and drop the lone `//!` — and apply it uniformly.

### [SHARED-14] OAuth is fully modeled but intentionally unwired (speculative surface)
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/shared/kernel/provider.rs:72` (`AuthMethod::Oauth`), `src/shared/kernel/provider.rs:181` (`Credential::Oauth`), `src/shared/kernel/provider.rs:185` (`OauthTokens`)
- **Problem:** `AuthMethod::Oauth`, `Credential::Oauth`, and the whole `OauthTokens` struct (with a vendor-specific `account_id` "required by the OpenAI/Codex backend") are never constructed outside tests, because subscription OAuth is intentionally unsupported (per the provider-auth ADR). This is ADR-sanctioned speculative generality, but it is worth a visibility flag against the "no speculative abstraction" rule, and the OpenAI/Codex-specific comment on a generic kernel type is a small coupling smell.
- **Evidence:**
```rust
/// `account_id` is required by the OpenAI/Codex backend (extracted from the id token).
pub struct OauthTokens {
    pub access: Secret,
    pub refresh: Secret,
    pub expires_at_ms: u64,
    pub account_id: Option<String>,
}
```
- **Recommendation:** Keep it (it is deliberate per ADR) but ensure the factory leaves such providers inert, and consider trimming `OauthTokens` to the minimal modeled shape until a sanctioned flow is implemented, dropping the vendor-specific `account_id` comment from the generic kernel type.

### [SHARED-15] Private-dir hardening is applied inconsistently and its failure is swallowed with an inaccurate justification
- **Severity:** Medium
- **Category:** security
- **Location:** `src/shared/infra/config.rs:677` (`let _ = ensure_private_dir(...)`), `src/shared/infra/config.rs:305` (`ensure_private_dir` 0700), `src/shared/infra/config.rs:368` (`update_global_config` uses plain `create_dir_all`)
- **Problem:** Two issues compound. (1) `Settings::resolve` does `let _ = ensure_private_dir(&global_dir)` with the justification "A real failure surfaces later when writing config or credentials." That is inaccurate for the permission-*coercion* path: if `~/.kiri` already exists at `0755` (e.g. created by an older version), `ensure_private_dir`'s `set_permissions(... 0o700)` failure is swallowed and writing a non-secret `config.toml` into the existing dir still succeeds — so a world-readable kiri dir can persist silently and never surface. (2) `update_global_config` creates the parent with plain `std::fs::create_dir_all(parent)` (default `0755`), not `ensure_private_dir`, so the dir-hardening that `resolve`/`write_starter_config` apply is bypassed on the writer path. The dir co-locates the `credentials.json` secret-fallback file, so this is a defense-in-depth gap, not just cosmetics.
- **Evidence:**
```rust
// resolve():
let _ = ensure_private_dir(&global_dir);
// update_global_config():
if let Some(parent) = config_path.parent() {
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow!("failed to create {}: {e}", parent.display()))?;
}
```
- **Recommendation:** Route every `~/.kiri` creation through `ensure_private_dir` (including `update_global_config`), and either surface the `ensure_private_dir` failure as a visible notice or correct the justification comment to reflect that a pre-existing world-readable dir is not re-detected later.

## Strengths
- `Secret` is a model of secure handling: `Zeroizing` inner, `Debug` redacted to `Secret(***)`, serialization confined to the keyring/0600 sink, and a tested `expose()` contract (`provider.rs:194-239`).
- The untrusted-project-layer boundary is real and regression-tested: `resolve_layers` honors only `effort` from the workspace and the test `resolve_layers_takes_only_effort_from_the_untrusted_workspace` proves a malicious repo cannot redirect a credential `base_url` or weaken the sandbox (`config.rs:291-301`, `config.rs:861-910`).
- Forward-compatible, fail-soft parsing is done well: `AuthMethod::Unknown` and the `ProviderKind` aliases let a config written by a newer Kiri load without aborting the boot, with byte-identical re-serialization, all under test (`provider.rs:97-107`, `provider.rs:298-332`).
- HTTP `connect`/`read` timeouts are defined as named, documented constants with a clear streaming-safety rationale, satisfying the "all I/O has timeouts" mandate at the source (`config.rs:131-138`).
