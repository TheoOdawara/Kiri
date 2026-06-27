# Audit — Architecture & Layer Invariants (cross-cutting)

> Scope: the whole `src/` tree — every module root (`agent`, `provider`, `tools`, `tui`, `memory`, `session`, `sync`), `shared/{kernel,infra}`, `main.rs`, `app.rs`, `characterization.rs`. Focus: dependency directions between `domain`/`application`/`infrastructure` across all modules, and the stated invariants as a whole-graph view.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary

The hexagonal layering is, on the whole, real and disciplined: every port is a capability-named trait (no `I`-prefix), domains and `shared/kernel` are genuinely I/O-free, all HTTP `send`/`bytes_stream` calls live in `provider/infrastructure`, all process spawning lives in `tools/infrastructure/exec` or the `sync` git adapter, and the security-critical "project config layer contributes only `effort`" invariant is implemented cleanly and regression-tested. The defects this whole-graph view surfaces are narrow but real: one port (`tools::application::Sandbox`) returns `anyhow::Result` instead of `AgentError`, directly violating the "ports return `AgentError`" invariant and diverging from its own sibling port; the `sync` application service performs filesystem I/O inline instead of behind an adapter; `shared/infra/config` reaches *down* into a module's infrastructure (`tools::infrastructure::sensitive`), inverting the intended dependency direction; and credential-resolution + provider wiring is duplicated/split between `app.rs` and the TUI runtime, which has quietly become a second composition root.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 4 | 1 |

## Compliance table

| Invariant | Status | Evidence |
|---|---|---|
| Network I/O only in `provider/infrastructure` (sync may shell to git) | **Pass** | All `.send()`/`.bytes_stream()` are in `provider/infrastructure/{openai,anthropic}` (`openai/provider.rs:83,99`; `anthropic/provider.rs:87,101`; `openai/embeddings.rs:90`). TUI holds a `reqwest::Client` but never sends (see ARCH-06). |
| Process spawning only in `tools` exec + `sync` git adapter | **Pass** | `tools/infrastructure/exec.rs:154` (single spawn), `confine/macos.rs:57` (decorator), `sync/infrastructure/git_cli.rs:31` (git, allowed by ADR 0015). |
| Filesystem/SQLite I/O only in `tools` FsSandbox + allowed owners (memory/session/sync data dirs, provider secrets, shared/infra config) | **Partial** | Owners are respected, but `sync/application/sync_service.rs` does fs I/O in the **application** layer, not infrastructure (ARCH-03). |
| `domain` has no I/O | **Pass** | Grep of every `*/domain` + `shared/kernel` for fs/net/io/process: only hit is `std::io::Error` as a `#[from]` *type* in `shared/kernel/error.rs:15`, not an I/O call. |
| Engine never touches stdin/stdout directly (all UI via ports) | **Pass** | `stdout()` only at `app.rs:48` (boot TTY check), `main.rs:43` (headless `kiri sync` summary), and `tui/infrastructure/{runtime,terminal_guard}` (the front-end). No prints in `agent`/`provider`/`tools`/`memory`/`session` engine paths. |
| Ports return `AgentError`; `anyhow` only at the binary edge | **Violated** | `tools::application::Sandbox` returns `anyhow::Result` (ARCH-01); `tools::infrastructure::sensitive` + the FsSandbox adapter use `anyhow` (ARCH-02). Boot/front-end edges (`config.rs`, `runtime.rs::run`) use `anyhow` defensibly. |
| Ports are traits, capability-named, no `I`-prefix | **Pass** | Grep for `pub trait I[A-Z]`: none. All 18 port traits are capability-named (`Sandbox`, `CommandSandbox`, `Tool`, `CompletionProvider`, `SecretStore`, `MemoryPort`, `SessionStore`, `Git`, `Presenter`, `ApprovalPolicy`, …). |
| Project config layer contributes **only** `effort` | **Pass** | `shared/infra/config.rs:298-300` `resolve_layers` returns `(global, effort)` unchanged; regression test `resolve_layers_takes_only_effort_from_the_untrusted_workspace` (line 862). |
| `shared` is depended-upon, not depending on modules | **Violated** | `shared/infra/config.rs:11-12` imports from `modules/tools` (incl. `infrastructure::sensitive`, a concrete adapter) (ARCH-04). |
| Single composition root | **Partial** | `app.rs::wire` is the documented root, but `tui/infrastructure/runtime.rs` is a second one (ARCH-06). |
| Each module folder = `domain`/`application`/`infrastructure` | **Pass (by design)** | `memory`/`session`/`sync`/`tui` have all three; `provider`/`tools` have `application`+`infrastructure` (pure data lives in `application`); `agent` has only `application` (its data lives in `shared/kernel`, per the contract). Consistent with CLAUDE.md. |

## Findings

### [ARCH-01] `Sandbox` port returns `anyhow::Result` instead of `AgentError`
- **Severity:** High
- **Category:** architecture
- **Location:** `src/modules/tools/application/sandbox.rs:3`, `src/modules/tools/application/sandbox.rs:34`, `src/modules/tools/application/sandbox.rs:38`
- **Problem:** The architecture invariant is explicit: *"Ports return `AgentError`; `anyhow` only at the binary edge."* The `Sandbox` port — one of the most central ports in the system, the filesystem chokepoint every tool resolves paths through — declares its fallible methods with `anyhow::Result`. This is a direct invariant violation, and it is **inconsistent with its own sibling port in the very same folder**: `CommandSandbox` (`command_sandbox.rs:32-36`) correctly returns `Result<_, AgentError>`. Two ports side-by-side, two different error contracts. The leak is functionally contained today only because each tool stringifies the error at the boundary (`read_file.rs:64` `Err(error) => return ToolOutcome::Error(error.to_string())`), but the contract — and a future caller that tries to `?`-propagate — is broken.
- **Evidence:**
```rust
// src/modules/tools/application/sandbox.rs
use anyhow::Result;
// ...
pub trait Sandbox {
    fn resolve_existing(&self, rel: &str) -> Result<PathBuf>;        // anyhow::Result
    fn resolve_create(&self, rel: &str) -> Result<CreateResolution>; // anyhow::Result
// vs the sibling port, same folder:
// command_sandbox.rs
    fn confine(&self, cmd: Command, policy: &SandboxPolicy)
        -> Result<tokio::process::Command, AgentError>;             // AgentError ✔
```
- **Recommendation:** Change the `Sandbox` port (and the `FsSandbox` adapter that implements it) to return `Result<_, AgentError>` — mapping canonicalization/traversal/sensitive failures to a suitable variant (e.g. a new `AgentError::Sandbox`-style path error, or reuse `Sandbox`/`Io`). This also lets tools propagate via `?` rather than stringifying. Do not implement here — scan-only.

### [ARCH-02] `anyhow` used off the binary edge in `tools` infrastructure
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/tools/infrastructure/sensitive.rs:3`, `src/modules/tools/infrastructure/sensitive.rs:52`, `src/modules/tools/infrastructure/sensitive.rs:109`, `src/modules/tools/infrastructure/sandbox.rs:4`, `src/modules/tools/infrastructure/sandbox.rs:67`
- **Problem:** Beyond the port itself (ARCH-01), `anyhow` has spread through the `tools` adapter layer: `SensitiveMatcher::new`, `load_sensitive_matcher`, and the whole `FsSandbox::with_confinement` constructor return `anyhow::Result`/use `Context`/`bail!`. Infrastructure adapters are supposed to map concrete failures into typed `AgentError` (per `error.rs:1`: *"Adapters map their concrete failures … into a variant"*). This is the same divergence as ARCH-01 spreading to the helpers, and it is what forces ARCH-04 (config imports the `anyhow`-typed `load_sensitive_matcher`). Note the two defensible-edge uses for contrast: `tui/infrastructure/runtime.rs:8` (`run()` is the outermost UI loop returned to `main`) and `shared/infra/config.rs` (the boot/config helper `Settings::resolve` is called directly from `main.rs:23`) — both sit at the front-end/boot edge and are acceptable; the `tools` adapter uses are not.
- **Evidence:**
```rust
// src/modules/tools/infrastructure/sensitive.rs
pub fn new(globs: &[&str]) -> Result<Self> {            // anyhow::Result
    let patterns = compiled.map_err(|e| anyhow!("invalid sensitive pattern: {e}"))?;
// src/modules/tools/infrastructure/sandbox.rs
let canonical = std::fs::canonicalize(root)
    .with_context(|| format!("sandbox root {} does not exist", root.display()))?;
```
- **Recommendation:** Convert the `tools` adapter constructors/helpers to `Result<_, AgentError>` once ARCH-01 lands; keep `anyhow` only in `main.rs`, `app.rs`, and arguably the boot/front-end edges (`config.rs`, `runtime.rs::run`). Scan-only.

### [ARCH-03] `sync` application service performs filesystem I/O inline, not via an adapter
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/modules/sync/application/sync_service.rs:5`, `src/modules/sync/application/sync_service.rs:61`, `src/modules/sync/application/sync_service.rs:127`, `src/modules/sync/application/sync_service.rs:200-205`, `src/modules/sync/application/sync_service.rs:239-248`
- **Problem:** `SyncService` lives in the `application` layer and abstracts its **git** dependency behind the `Git` port (good) and its **memory** export behind `sync::infrastructure::memory_ndjson` (good) — but it does its **filesystem** work inline with `tokio::fs` (`create_dir_all`, `write`, `read_to_string`, `copy`, `rename`, plus a private `write_atomic`). The result is a split-personality layer: two of three external dependencies are behind adapters, the third is raw I/O in the use-case. The `sync` context is an allowed fs owner, so this is not an invariant *violation*, but it is a layer-purity inconsistency that the whole-graph view exposes: the application layer is the one place use-cases should stay I/O-free, and the module already has a `sync/infrastructure/` folder (`memory_ndjson.rs`) that is the natural home.
- **Evidence:**
```rust
// src/modules/sync/application/sync_service.rs
use tokio::fs;                                   // I/O in the application layer
// ...
let incoming = fs::read_to_string(&incoming_config).await?;
// ...
fs::copy(&self.config_path, dir.join(CONFIG_FILE)).await?;
// ...
async fn write_atomic(path: &Path, contents: &str) -> Result<()> {  // adapter logic in application
    fs::write(&tmp, contents).await?;
    fs::rename(&tmp, path).await?;
```
- **Recommendation:** Move the work-tree/config file operations (`write_atomic`, the gitignore/config copy, the incoming-config read) into a small `sync/infrastructure` file system adapter (a port the service consumes), mirroring how `memory_ndjson` and the `Git` port are already structured. Scan-only.

### [ARCH-04] `shared/infra/config` depends on a module's infrastructure (inverted direction)
- **Severity:** Medium
- **Category:** architecture
- **Location:** `src/shared/infra/config.rs:11-12`, `src/shared/infra/config.rs:612`, `src/shared/infra/config.rs:722`
- **Problem:** `shared/` is described in the contract as *"cross-cutting primitives … shared across `agent`, `provider`, `session`, `memory`, `config`, and `tui`"* — i.e. a foundation modules depend **on**. Yet `shared/infra/config.rs` imports `SensitiveMatcher` and `load_sensitive_matcher` from `modules::tools::infrastructure::sensitive` (a concrete *adapter*), and stores `sensitive: SensitiveMatcher` directly on `Settings`. This points the dependency arrow the wrong way: a shared/foundation module reaches down into one specific module's infrastructure. It also couples config to a tools adapter type and is what propagates ARCH-02's `anyhow` into `Settings::resolve`. (The sibling import of `NetworkPolicy` from `tools::application::command_sandbox` is a pure-data enum and milder, but still `shared → module`.)
- **Evidence:**
```rust
// src/shared/infra/config.rs
use crate::modules::tools::infrastructure::sensitive::{SensitiveMatcher, load_sensitive_matcher};
// ...
pub struct Settings {
    pub sensitive: SensitiveMatcher,   // a tools-infra adapter type held by shared config
// ...
    sensitive: load_sensitive_matcher()?,   // shared calling into tools::infrastructure
```
- **Recommendation:** Have `Settings` carry the *raw* sensitive patterns (e.g. `Vec<String>` / `Option<String>` from env) and let the composition root (`app.rs::wire`) build the `SensitiveMatcher` and hand it to `FsSandbox`. Or relocate `SensitiveMatcher`'s pattern type to `shared/kernel` if it is genuinely cross-cutting. Either way config stops depending on a module's adapter. Scan-only.

### [ARCH-05] Credential-resolution logic duplicated across `app.rs` and the TUI `ProviderSwap`
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/app.rs:390-424`, `src/modules/tui/infrastructure/runtime.rs:144-167`
- **Problem:** The exact rule for resolving a provider credential — *keyless short-circuit → stored secret → one-time env-var import (`api_key_from_env`) with best-effort persist* — is implemented twice, once in `app.rs::resolve_credential` (boot) and once in `ProviderSwap::resolve_credential` (live `/provider` switch). They already diverge in subtle ways (boot returns `Ok(None)` for "nothing configured → onboarding" and distinguishes a store fault; the runtime returns an `AgentError::Provider` "no credential" message). Two copies of a security-sensitive rule guarantee they drift further — a fix to one (e.g. tightening which env vars are honored, or how a stale keyless key is cleared) silently misses the other.
- **Evidence:**
```rust
// app.rs:397         vs    // runtime.rs:148
if profile.auth == AuthMethod::None { return Ok(Some(Credential::None)); }   // app
if profile.auth == AuthMethod::None { return Ok(Credential::None); }          // runtime
// both then: secrets.get(&profile.id)? → api_key_from_env(profile) → secrets.set(best-effort)
```
- **Recommendation:** Extract one credential-resolution function (in `provider` — e.g. alongside `factory`/`SecretStore`) returning a single shared shape, and have both the composition root and `ProviderSwap` call it; let each caller map the "absent" case to its own policy (onboarding vs error) at the call site. Scan-only.

### [ARCH-06] TUI runtime has become a second composition root
- **Severity:** Low
- **Category:** architecture
- **Location:** `src/modules/tui/infrastructure/runtime.rs:28`, `src/modules/tui/infrastructure/runtime.rs:80`, `src/modules/tui/infrastructure/runtime.rs:1370`, `src/modules/tui/infrastructure/runtime.rs:1389-1390`, `src/modules/tui/infrastructure/runtime.rs:1463`
- **Problem:** `app.rs::wire` is documented as *"the one place adapters are chosen."* In practice `tui/infrastructure/runtime.rs` also constructs adapters: it holds a `reqwest::Client` + `Box<dyn SecretStore>` and calls `build_provider` for live swaps (`ProviderSwap`), and for the `/sync` command it instantiates `SqliteSharedMemory::new`, `GitCli`, `SyncService::new`, and a `Distiller`. This is defensible (live provider swap and on-demand sync genuinely need to build things after boot), and it stays within the `infrastructure` layer so it is not an I/O-layer leak — but it means the wiring graph and the "single composition root" intent are split across two files, and it is the structural reason ARCH-05 exists. Holding the bare `reqwest::Client` here is also why the network-I/O grep lights up in `tui` (no request is ever sent here — the client is only forwarded to `build_provider`).
- **Evidence:**
```rust
// src/modules/tui/infrastructure/runtime.rs
use crate::modules::provider::infrastructure::factory::{api_key_from_env, build_provider};
pub struct ProviderSwap { client: reqwest::Client, secrets: Box<dyn SecretStore>, /* ... */ }
// ...
let memory = match SqliteSharedMemory::new(shared_db) { /* ... */ };
let git = GitCli;
let service = SyncService::new(&git, global_dir, config_path.to_path_buf(), &memory);
```
- **Recommendation:** Accept this as an intentional "runtime sub-factory" but make it explicit — give `ProviderSwap` and the `/sync` path a single injected factory/closure built in `app.rs` (so adapter *choice* stays in one place and the runtime only *invokes* it), and resolve ARCH-05 in the same move. Scan-only.

## Strengths
- **Ports are textbook:** 18 capability-named trait ports, zero `I`-prefix, object-safe where they need to be (`&dyn Sandbox`, `&dyn CommandSandbox`); the `CommandSandbox` port even documents *why* it decorates-not-spawns to preserve the single spawn site.
- **The hard invariants hold:** domains + `shared/kernel` are genuinely I/O-free; every HTTP `send` is in `provider/infrastructure`; every process spawn is in `tools/infrastructure/exec` or the sync git adapter; the engine reaches the terminal only through the TUI front-end.
- **The security-critical config-layering invariant is exemplary:** `resolve_layers` takes *only* `effort` from the untrusted workspace layer, is a pure function, carries a "why" comment tying it to the credential-exfiltration threat, and is locked down by a named regression test.
