# Audit — tools module

> Scope: `src/modules/tools.rs`; application (`application.rs`, `command_sandbox.rs`, `plan.rs`, `registry.rs`, `sandbox.rs`, `tool.rs`); infrastructure (`infrastructure.rs`, `args.rs`, `exec.rs`, `sensitive.rs`, `support.rs`, `sandbox.rs`, `confine.rs` + `confine/{macos,noop}.rs`, `control.rs` + `control/present_plan.rs`, and all 10 `fs/*` tools).  
> Date: 2026-06-27  
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The tools module is the security core of Kiri and is in good shape overall: the sandbox path-resolution guards are thorough and well-tested (traversal, symlink-escape, sensitive-name, credential-directory, out-of-root cases all have dedicated tests), no `unwrap`/`expect`/`panic!` is reachable outside `#[cfg(test)]`, every spawned process is timeout-bounded with `kill_on_drop`, and the `Tool`/`ToolRegistry`/`Sandbox`/`CommandSandbox` port design is clean. The headline issue is one architecture-invariant violation: the `Sandbox` application port returns `anyhow::Result` instead of `AgentError`, inconsistent with its sibling `CommandSandbox` port. The remaining findings are maintainability: notable cross-file duplication (the `exec::run_argv` match-arm shape, the per-tool confirmation builder, the security-critical `SECRET_DIRS` list duplicated in two files), a triple-sourced 30-second default, and `edit_file` performing un-timed blocking `std::fs` I/O against the contract's "all I/O has a timeout" rule. No critical correctness or sandbox-escape defect was found.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 5 | 6 |

## Findings

### [TOOL-01] `Sandbox` application port returns `anyhow::Result` instead of `AgentError`
- **Severity:** High
- **Category:** architecture
- **Location:** `src/modules/tools/application/sandbox.rs:3`, `src/modules/tools/application/sandbox.rs:34`, `src/modules/tools/application/sandbox.rs:38` — contrast `src/modules/tools/application/command_sandbox.rs:36`
- **Problem:** The project invariant is explicit: "Ports return `AgentError`; `anyhow` only at the binary edge." `Sandbox` is an application-layer port (a capability trait consumed by every tool and the registry), yet its fallible methods return `anyhow::Result`. Its sibling port in the same package, `CommandSandbox`, correctly returns `Result<_, AgentError>`. This both violates the layer invariant and makes the two ports inconsistent. Because callers only stringify the error (`error.to_string()`), the typed-error contract is lost at the boundary — a caller cannot match on a sandbox error kind.
- **Evidence:**
```rust
// application/sandbox.rs
use anyhow::Result;
// ...
fn resolve_existing(&self, rel: &str) -> Result<PathBuf>;
fn resolve_create(&self, rel: &str) -> Result<CreateResolution>;

// application/command_sandbox.rs (sibling port, the correct shape)
fn confine(
    &self,
    cmd: tokio::process::Command,
    policy: &SandboxPolicy,
) -> Result<tokio::process::Command, AgentError>;
```
- **Recommendation:** Change the `Sandbox` port methods to return `Result<_, AgentError>` (e.g. an `AgentError::Sandbox`/`AgentError::Io` variant), and adapt the `FsSandbox` adapter to map its internal `anyhow` errors at the adapter boundary. Inherent constructors (`with_confinement`, `new`, `relocated`) are wiring and may keep `anyhow`, but the consumed port should not.

### [TOOL-02] `exec::run_argv` result-match + error-format shape duplicated across six fs tools
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/tools/infrastructure/fs/write_file.rs:86`, `src/modules/tools/infrastructure/fs/delete_file.rs:79`, `src/modules/tools/infrastructure/fs/delete_dir.rs:84`, `src/modules/tools/infrastructure/fs/create_dir.rs:69`, `src/modules/tools/infrastructure/fs/list_dir.rs:70`, `src/modules/tools/infrastructure/fs/move_path.rs:99`
- **Problem:** Every unix file tool repeats the same `run_argv(...).await` three-arm match — `Ok(result) if result.succeeded()` → success, `Ok(result)` → `format!("cannot <verb> {}: {}", path, result.stderr_text())`, `Err(error)` → `format!("cannot <verb> {}: {}", path, error.message())`. The only variation is the verb and the success message. Six near-identical copies of a security-relevant code path raise the chance one drifts (e.g. one forgets the `succeeded()` arm and treats a non-zero exit as success).
- **Evidence:**
```rust
// delete_file.rs (representative; create_dir / delete_dir / list_dir / write_file / move_path match)
Ok(result) if result.succeeded() => ToolOutcome::Ok(format!("deleted {}", args.path)),
Ok(result) => ToolOutcome::Error(format!(
    "cannot delete {}: {}", args.path, result.stderr_text()
)),
Err(error) => {
    ToolOutcome::Error(format!("cannot delete {}: {}", args.path, error.message()))
}
```
- **Recommendation:** Add a shared helper in `infrastructure/exec.rs` or `support.rs`, e.g. `run_fs_argv(sandbox, argv, cwd, verb: &str, label: &str, ok: impl FnOnce() -> String) -> ToolOutcome`, that owns the policy build, the spawn, and the three-arm mapping; each tool then supplies only argv + verb + success text.

### [TOOL-03] Per-tool `confirmation` builder duplicated across seven tools
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/tools/infrastructure/fs/read_file.rs:48`, `src/modules/tools/infrastructure/fs/edit_file.rs:41`, `src/modules/tools/infrastructure/fs/delete_file.rs:44`, `src/modules/tools/infrastructure/fs/list_dir.rs:43`, `src/modules/tools/infrastructure/fs/create_dir.rs:44`, `src/modules/tools/infrastructure/fs/delete_dir.rs:45`, `src/modules/tools/infrastructure/fs/search.rs:84`
- **Problem:** The single-path tools all repeat the identical confirmation skeleton: parse the args, build `command_line`, then `Some(confirm(format!("<pt-BR phrase>. Aprova executar: {cmd}?"), default_accept_for(&a.path)))`. Only the phrase differs. The duplication is benign today but means the "phrase + suffix + default-accept rule" lives in seven places rather than one.
- **Evidence:**
```rust
// create_dir.rs (representative)
fn confirmation(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> Option<Confirmation> {
    let cmd = self.command_line(sandbox, call)?;
    let a: PathArgs = parse(call.function.arguments.as_str()).ok()?;
    Some(confirm(
        format!("Criar diretório. Aprova executar: {cmd}?"),
        default_accept_for(&a.path),
    ))
}
```
- **Recommendation:** Provide a shared helper alongside `confirm` in `application/tool.rs`, e.g. `simple_path_confirmation(sandbox, call, phrase: &str)` that does the parse + command_line + default-accept assembly; tools that need bespoke prose (`write_file`, `move_path`) keep their own.

### [TOOL-04] Security-critical secret-directory list duplicated in two files
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/tools/infrastructure/sandbox.rs:19` (`SECRET_DIRS`) and `src/modules/tools/infrastructure/confine/macos.rs:13` (`SECRET_HOME_DIRS`)
- **Problem:** Two byte-identical lists of credential directory names back two independent enforcement layers (path-resolution refusal vs. the Seatbelt `deny file-read*` rules). They are a single security policy expressed twice; if one is extended (e.g. add `.config/gcloud`) and the other is not, a confined command could still read a store the resolver blocks, or vice-versa. The duplication is silent — nothing links them.
- **Evidence:**
```rust
// sandbox.rs
pub(crate) const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"];
// confine/macos.rs
const SECRET_HOME_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"];
```
- **Recommendation:** Define the list once (e.g. export `SECRET_DIRS` from `sandbox.rs` and have `macos.rs` consume it, or hoist it to a shared `tools` constants module) so both layers are guaranteed to agree.

### [TOOL-05] The 30-second default timeout (and "64 KiB" cap) is sourced from three unrelated literals
- **Severity:** Medium
- **Category:** magic
- **Location:** `src/modules/tools/infrastructure/args.rs:60` (`default_timeout_ms` → `30_000`), `src/modules/tools/infrastructure/exec.rs:20` (`DEFAULT_TIMEOUT = 30s`), `src/modules/tools/infrastructure/fs/run_command.rs:104`, `src/modules/tools/infrastructure/fs/run_command.rs:121` (schema `"Defaults to 30000"` / `"default 30s"`)
- **Problem:** "30 seconds" appears as `30_000` (the serde default), as `Duration::from_secs(30)` (the file-tool bound, unrelated to run_command), and twice as a hardcoded string/number in the `run_command` schema description. They are not linked, so changing the default in one place leaves the advertised description and/or the file-tool bound stale. Likewise the "64 KiB" output cap is a named constant (`EXEC_MAX_BYTES`) in `exec.rs` but a free-text "`64 KiB`" string in the `run_command` schema (`run_command.rs:104`).
- **Evidence:**
```rust
// args.rs
fn default_timeout_ms() -> u64 { 30_000 }
// run_command.rs schema
"description": "Timeout in milliseconds. Defaults to 30000.",
"default": 30000,
// ...same file: "Output is truncated at 64 KiB. A timeout (default 30s) ..."
```
- **Recommendation:** Define a single `RUN_COMMAND_DEFAULT_TIMEOUT_MS` constant and reference it from the serde default; build the schema description via `format!` over that constant and `EXEC_MAX_BYTES` so the advertised text cannot drift from the enforced value.

### [TOOL-06] `edit_file` performs un-timed blocking `std::fs` I/O on the async runtime
- **Severity:** Medium
- **Category:** error-handling
- **Location:** `src/modules/tools/infrastructure/fs/edit_file.rs:79`, `src/modules/tools/infrastructure/fs/edit_file.rs:92`
- **Problem:** The contract requires "All I/O has a timeout … and any blocking await. A hung dependency must fail fast." `edit_file::execute` is `async` but calls `std::fs::read_to_string` and `std::fs::write` directly — synchronous, un-timed, on the single-threaded TUI runtime. Every other tool either shells out through the timeout-bounded `exec::run_argv` or guards with `stat_guard`'s `tokio::time::timeout`. On a wedged/stale network mount the read or write can block the entire runtime indefinitely, exactly the hang class the contract calls out. The preceding `stat_guard` is timeout-bounded, but the actual read/write that follows it is not.
- **Evidence:**
```rust
let content = match std::fs::read_to_string(&path) {   // blocking, no timeout
    Ok(content) => content,
    Err(error) => return ToolOutcome::Error(format!("cannot read {} as text: {error}", args.path)),
};
// ...
match std::fs::write(&path, updated.as_bytes()) {       // blocking, no timeout
```
- **Recommendation:** Wrap the read/write in `tokio::time::timeout(exec::DEFAULT_TIMEOUT, tokio::fs::…)` (mirroring `stat_guard`), or route the splice through the existing exec path. Note the `Tool` doc-comment claims the fast file tools "complete in microseconds" — that assumption fails on a stale mount, so the timeout is the correct guard.

### [TOOL-07] Read-only tools add their working dir to the *write* allow-list of the OS policy
- **Severity:** Low
- **Category:** security
- **Location:** `src/modules/tools/infrastructure/fs/read_file.rs:85`, `src/modules/tools/infrastructure/fs/list_dir.rs:82`, `src/modules/tools/infrastructure/fs/search.rs:132`
- **Problem:** `read_file`, `list_dir`, and `search` pass their resolved cwd as the per-call `extra_rw` argument to `command_policy`, which lands it in the Seatbelt `file-write*` allow-list. These operations only read (`head`/`ls`/`grep`), and `current_dir` does not require write permission, so the grant is unnecessary and weakens least-privilege for an out-of-root read target (the base profile already permits reads everywhere except secret dirs). It is not exploitable through the fixed argv, but it broadens the confinement beyond the operation.
- **Evidence:**
```rust
// read_file.rs — a read passes its cwd as extra_rw (write allow-list)
&sandbox.command_policy(NetworkPolicy::Deny, &[&cwd]),
```
- **Recommendation:** Give `command_policy` (or a read variant) a way to pass read-only extras, and have the read-only tools pass `&[]` for `extra_rw` (their reads are already covered by the base profile). Only the mutating tools should add their cwd to the write allow-list.

### [TOOL-08] `ToolRegistry::is_destructive` is dead in production, kept with a speculative rationale
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/modules/tools/application/registry.rs:44`
- **Problem:** The method carries `#[allow(dead_code)]` and a comment stating it is "Currently unused in the engine path … kept as a classification test assertion and for future use." Its only caller is the test `is_destructive_classifies_tools`. The contract is explicitly anti-speculative ("no speculative generality, no patterns without a present need"). A production method existing solely to be asserted, justified by "future use," is the pattern to avoid.
- **Evidence:**
```rust
#[allow(dead_code)]
pub fn is_destructive(&self, name: &str) -> bool {
    self.find(name).is_some_and(|tool| !tool.is_read_only())
}
```
- **Recommendation:** Either drop it (the underlying `Tool::is_read_only` is what tests actually need), or, if a destructive-warning feature is genuinely imminent, record it as a tracked item rather than carrying speculative dead code in the registry.

### [TOOL-09] `SensitiveMatcher::empty()` uses `#[allow(dead_code)]` where the sibling test helper uses `#[cfg(test)]`
- **Severity:** Low
- **Category:** inconsistency
- **Location:** `src/modules/tools/infrastructure/sensitive.rs:67` vs. `src/modules/tools/infrastructure/sandbox.rs:44`
- **Problem:** Two test-only constructors use two different conventions. `FsSandbox::new` (used only by tests) is gated `#[cfg(test)]`, which correctly excludes it from release builds. `SensitiveMatcher::empty()` (also used only by tests) is instead a normal `pub fn` annotated `#[allow(dead_code)]`, so it ships in release builds and the warning is merely suppressed. Pick one convention.
- **Evidence:**
```rust
// sensitive.rs
#[allow(dead_code)]
pub fn empty() -> Self { /* ... */ }
// sandbox.rs
#[cfg(test)]
pub fn new(root: impl AsRef<Path>, sensitive: SensitiveMatcher) -> Result<Self> { /* ... */ }
```
- **Recommendation:** Gate `empty()` with `#[cfg(test)]` to match `FsSandbox::new`, removing the `#[allow(dead_code)]` and keeping it out of release builds.

### [TOOL-10] `run_command` schema advertises a `minimum` timeout but not the enforced `maximum`
- **Severity:** Low
- **Category:** magic
- **Location:** `src/modules/tools/infrastructure/fs/run_command.rs:119` (schema) vs. `src/modules/tools/infrastructure/fs/run_command.rs:46`/`:48` (`RUN_COMMAND_MAX_TIMEOUT_MS` + `effective_timeout_ms` clamp)
- **Problem:** The advertised schema declares `"minimum": 1000` but no maximum, while `effective_timeout_ms` silently clamps any request into `[1_000, 600_000]`. A model asking for a 30-minute timeout is silently capped at 10 minutes with no feedback, and the schema gives no hint the ceiling exists — a small "no silent surprise on input" gap.
- **Evidence:**
```rust
"timeout_ms": {
    "type": "integer",
    "description": "Timeout in milliseconds. Defaults to 30000.",
    "default": 30000,
    "minimum": 1000
    // no "maximum", yet effective_timeout_ms clamps to 600_000
}
```
- **Recommendation:** Add `"maximum": 600000` to the schema (ideally interpolated from `RUN_COMMAND_MAX_TIMEOUT_MS`) so the advertised contract matches the enforced clamp.

### [TOOL-11] `sandbox.rs` mixes the `FsSandbox` adapter with free path/confirmation helpers
- **Severity:** Low
- **Category:** file-size
- **Location:** `src/modules/tools/infrastructure/sandbox.rs:319` (`expand_tilde`), `:333` (`home`), `:341` (`is_absolute_target`), `:347` (`default_accept_for`)
- **Problem:** `sandbox.rs` is ~663 lines (about half tests). Beyond the `FsSandbox` adapter and the `Sandbox` impl it also hosts four free functions — tilde expansion, HOME lookup, and the confirmation-default helpers (`is_absolute_target`/`default_accept_for`). The confirmation-default helpers are consumed by every fs tool's `confirmation`, not by the sandbox itself, so they read as a second responsibility colocated for convenience.
- **Evidence:**
```rust
pub(crate) fn is_absolute_target(path: &str) -> bool { /* used by every fs tool's confirmation */ }
pub(crate) fn default_accept_for(path: &str) -> bool { !is_absolute_target(path) }
```
- **Recommendation:** Extract the path/tilde + confirmation-default helpers into a small `infrastructure/path.rs` (or `confirm_default.rs`), leaving `sandbox.rs` focused on the `FsSandbox` adapter and its `Sandbox` impl.

### [TOOL-12] Stale test comment references a non-existent constant `RUN_COMMAND_MAX_BYTES`
- **Severity:** Low
- **Category:** dead-code
- **Location:** `src/modules/tools/infrastructure/fs/run_command.rs:464`
- **Problem:** A test comment says "Generate enough output to exceed RUN_COMMAND_MAX_BYTES," but no such constant exists; the actual cap is `exec::EXEC_MAX_BYTES` (asserted two lines later at `run_command.rs:477`). The comment names a symbol that was presumably renamed, which misleads a reader grepping for it.
- **Evidence:**
```rust
// Generate enough output to exceed RUN_COMMAND_MAX_BYTES. The shell loop syntax differs ...
// ...
assert!(text.len() <= exec::EXEC_MAX_BYTES + 200);
```
- **Recommendation:** Update the comment to reference `exec::EXEC_MAX_BYTES`.

## Strengths
- The sandbox security model is genuinely well-built: lexical traversal rejection, canonicalization-based escape checks, the file-name sensitive-pattern guard, and the credential-directory guard are each covered by dedicated tests including the unix symlink-escape cases (`sandbox.rs:626-662`) — and the macOS Seatbelt adapter "fails closed" if `sandbox-exec` vanishes (`confine/macos.rs:41`).
- Process execution is centralized in one timeout-bounded, `kill_on_drop`, concurrent-stdin-draining `run` (`exec.rs:137`), and the failed-stdin-write surfacing (`exec.rs:176`) shows real attention to not swallowing partial writes — both with regression tests.
- The `Tool`/`ToolRegistry` design is clean and extensible exactly as the architecture promises (one file per tool, capability-named ports, no `I`-prefix), and `present_plan` correctly models a plan-only control surface withheld from non-plan modes with a total, panic-free `execute`.
