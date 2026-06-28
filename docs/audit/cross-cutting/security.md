# Audit — Security sweep (cross-cutting)

> Scope: secrets handling (`shared/kernel/provider.rs`, `provider/infrastructure/secrets/{file_store,keyring_store}.rs`, `secret_store.rs`, `app.rs`); the filesystem sandbox + path confinement (`tools/infrastructure/sandbox.rs`, `tools/application/{sandbox,command_sandbox}.rs`, `tools/infrastructure/fs/{run_command,search}.rs`, `tools/infrastructure/{exec,sensitive,args}.rs`); OS confinement (`tools/infrastructure/confine/{macos,noop}.rs`, `confine.rs`); provider HTTP + SSE/JSON parsing (`provider/infrastructure/{openai,anthropic}/{provider,sse}.rs`, `http_error.rs`, `factory.rs`, `openai/arguments.rs`, `openai/embeddings.rs`, `unconfigured.rs`); config trust layering (`shared/infra/config.rs`); sync (`sync/infrastructure/{git_cli,memory_ndjson}.rs`, `sync/application/{git,sync_service}.rs`, `main.rs`); plus a whole-tree grep for `unsafe`, `unwrap/expect/panic`, and debug prints.
> Date: 2026-06-27
> Pass: read-only multi-agent sweep (scan phase — findings only, no code changed)

## Summary
The secrets subsystem is genuinely strong: `Secret` zeroizes on drop, redacts in `Debug`, and is never written to TOML; the untrusted project config layer is reduced to the single `effort` field; the sync trust-gate catches credential-redirect and sandbox-weakening; HTTP has connect+read timeouts and SSE/JSON parsing is panic-free with a streamed-byte cap. No `unsafe`, no runtime-reachable `unwrap/expect/panic`, no secret logging. The headline cross-cutting weakness is an asymmetry the per-module passes can't see: the carefully-built sensitive-file / credential-directory chokepoint guards the *file tools* but `run_command` is guarded only by the OS sandbox — which is macOS-only, narrower than the sensitive-name list, and does not protect Kiri's own credential store (`~/.kiri/credentials.json`) or common home credential dotfiles. The OS deny-list is also duplicated verbatim across two modules and has drifted from the sensitive-name list.

## Severity rollup
| Critical | High | Medium | Low |
|---|---|---|---|
| 0 | 1 | 2 | 4 |

## Findings

### [SEC-01] `run_command` bypasses the sensitive-file / secret-dir chokepoint; only the OS sandbox guards it
- **Severity:** High
- **Category:** security
- **Location:** `src/modules/tools/infrastructure/fs/run_command.rs:148`-`186`, `src/modules/tools/infrastructure/exec.rs:90`-`113`, `src/modules/tools/infrastructure/confine/noop.rs:10`-`22`
- **Problem:** The file tools resolve every path through `FsSandbox::resolve_existing`/`resolve_create`, which enforce `assert_not_sensitive` + `assert_not_in_secret_dir` (e.g. `read_file .env` and `read_file ~/.aws/config` are refused). `run_command` validates **only its `cwd`** through `resolve_existing`; the command string itself is arbitrary shell run via `run_shell`, with no content/secret guard. So `run_command` with `cat .env`, `cat ~/.aws/credentials`, or `cat ~/.npmrc` reads exactly the material the file-tool guard exists to block, and the bytes are returned to the model as the tool result (and rendered in the transcript / sent to the provider). The only compensating control is the OS sandbox — and on Linux/Windows that is `NoConfinement` (a pass-through), so on those platforms `run_command` has **zero** secrets protection. This is the single most consequential cross-cutting gap: it routes around the entire secrets-chokepoint investment. Mitigation is real but partial: `run_command.confirm_in_auto()` returns `true` (`run_command.rs:209`), so the command is shown and user-approved even in auto mode — yet a user can approve a benign-looking command (`npm run build`, `make`) that internally reads a workspace `.env`, and the secret still flows to the model.
- **Evidence:**
```rust
async fn execute(&self, sandbox: &dyn Sandbox, call: &ToolCall) -> ToolOutcome {
    let args: RunCommandArgs = ...;
    // only the cwd is path-validated; the command body is arbitrary shell
    let cwd = match sandbox.resolve_existing(&args.cwd) { ... };
    let result = match exec::run_shell(&args.command, Some(&cwd), ...).await { ... };
```
- **Recommendation:** Decide and document the intended boundary, then close it: either (a) treat `run_command` as outside the secrets guarantee and say so prominently in the system prompt / docs, or (b) add a lightweight pre-exec heuristic that flags commands reading a sensitive path and forces an explicit, scarier confirmation, and (c) make OS confinement deny-read the same set the file-tool guard blocks (see SEC-02) so at least the macOS path is consistent. Do not implement here — scan only.

### [SEC-02] OS read-deny list omits Kiri's own credential store and common home credential files
- **Severity:** Medium
- **Category:** security
- **Location:** `src/modules/tools/infrastructure/confine/macos.rs:13`, `macos.rs:86`-`122`, `src/shared/infra/config.rs:744`, `src/modules/tools/infrastructure/sandbox.rs:19`
- **Problem:** The macOS Seatbelt profile uses a permissive base (`(allow default)`) and only adds read-denies for `SECRET_HOME_DIRS = [.ssh, .aws, .gnupg, .gpg, .kube, .docker]`. It does **not** deny `~/.kiri` — which holds `credentials.json`, the 0600 API-key fallback store (`config.rs:744` → `global_dir.join("credentials.json")`) used whenever the OS keyring is unreachable — nor common home credential files (`~/.netrc`, `~/.npmrc`, `~/.git-credentials`, `~/.config/gh/hosts.yml`, `~/.config/...` tokens). So a confined `run_command` can `cat ~/.kiri/credentials.json` and return Kiri's own stored provider keys to the model. The file-tool guard *does* block `credentials.json` (it matches the `credentials.*` sensitive pattern, `sensitive.rs:23`) — making the omission an inconsistency: the same file is protected for `read_file` but readable via `run_command`. Network-deny limits direct exfiltration, but the secret reaching the model/transcript is already a disclosure. Mitigated by: macOS usually stores secrets in Keychain (not the file), and `run_command` is always user-confirmed.
- **Evidence:**
```rust
const SECRET_HOME_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"];
// build_profile: base is permissive, only these dirs are read-denied
let mut profile = String::from("(version 1)\n(allow default)\n");
...
for dir in SECRET_HOME_DIRS { push_deny_read(&mut profile, &home.join(dir)); }
```
- **Recommendation:** Add `~/.kiri` and the well-known home credential files/dirs to the OS read-deny set, and derive the deny set from the same source of truth as the file-tool guard so coverage cannot diverge (see SEC-03). Consider tightening the Seatbelt base from `(allow default)` to deny-reads-by-default with explicit workspace + toolchain read-allows, since the current base means the deny-list is the *only* thing standing between a confined command and the whole filesystem.

### [SEC-03] Credential-directory list is duplicated and has drifted from the sensitive-name list
- **Severity:** Medium
- **Category:** duplication
- **Location:** `src/modules/tools/infrastructure/sandbox.rs:19`, `src/modules/tools/infrastructure/confine/macos.rs:13`, `src/modules/tools/infrastructure/sensitive.rs:10`-`40`
- **Problem:** `SECRET_DIRS` (sandbox.rs, used by the file tools' `secret_dir_component` / `search --exclude-dir`) and `SECRET_HOME_DIRS` (macos.rs, used by the Seatbelt read-deny) are byte-identical lists defined twice (`[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"]`). They are a single security policy enforced in two layers, so they must stay in sync, but nothing ties them together — a future edit to one silently weakens the other. Both also diverge from `DEFAULT_SENSITIVE_PATTERNS` (which covers many more names) and neither covers `~/.kiri`, `.git-credentials`, `.config`, etc. This duplication is the root cause that makes SEC-02 easy to introduce and hard to notice.
- **Evidence:**
```rust
// sandbox.rs
pub(crate) const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"];
// confine/macos.rs
const SECRET_HOME_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".gpg", ".kube", ".docker"];
```
- **Recommendation:** Hoist the credential-directory list to one shared constant (e.g. beside `DEFAULT_SENSITIVE_PATTERNS` or in the kernel) and reference it from both the file-tool guard and the Seatbelt profile, so the two enforcement layers cannot drift. Audit it against the broader sensitive-name list while consolidating.

### [SEC-04] `kiri sync init <url>` passes an unvalidated remote URL to `git`; `ext::`/`-`-prefixed URLs are risky
- **Severity:** Low
- **Category:** security
- **Location:** `src/main.rs:39`, `src/modules/sync/application/sync_service.rs:71`, `src/modules/sync/infrastructure/git_cli.rs:21`-`35`
- **Problem:** The CLI-supplied URL flows straight into `git remote add origin <url>` (and is later fetched/pushed). It is passed as a distinct argv element (no shell — so no classic command injection, good), but git's own transport layer treats `ext::sh -c '…'` (and other `ext::`/`fab::`/`-`-prefixed) remotes as code-execution vectors on fetch/push. The risk is bounded because the user supplies their own repo URL (self-inflicted, not attacker-reachable — the URL lives in `~/.kiri/sync/.git/config`, not in the synced `config.toml`), so this is hardening rather than an exploitable hole.
- **Evidence:**
```rust
self.git_ok(&["remote", "add", "origin", remote_url], &dir).await?;
```
- **Recommendation:** Validate the scheme on `init` — accept only `https://`, `git@…`/`ssh://`, and a local path; reject `ext::`/`fab::`/`file://-`/leading-`-` — and prefix positional URLs with `--` where git supports it. Document that the sync remote is trusted input.

### [SEC-05] API key is exposed into reqwest header buffers that are not zeroized
- **Severity:** Low
- **Category:** security
- **Location:** `src/modules/provider/infrastructure/openai/provider.rs:79`-`81`, `src/modules/provider/infrastructure/anthropic/provider.rs:84`, `src/modules/provider/infrastructure/openai/embeddings.rs:82`-`84`
- **Problem:** `Secret::expose()` correctly hands the key only to the auth-header call site, but `reqwest`'s `bearer_auth`/`header` copy the value into a `HeaderValue` (and internal request buffers) that are plain `String`/`Bytes` and are **not** zeroized — so a copy of the key lingers in heap memory for the request's lifetime, outside `Zeroizing`'s coverage. This is an inherent limitation of the HTTP client, not a defect in Kiri's `Secret` design, and the exposure window is short.
- **Evidence:**
```rust
if let Some(key) = &self.api_key {
    request = request.bearer_auth(key.expose());
}
```
- **Recommendation:** Accept and document as a known residual (the cleanest mitigation is upstream in reqwest). Nothing actionable in this codebase beyond keeping the `expose()` call sites minimal, which they already are.

### [SEC-06] Sensitive-pattern list is enumerated a second time inside the system prompt and drifts on override
- **Severity:** Low
- **Category:** duplication
- **Location:** `src/shared/infra/config.rs:105`-`112`, `src/modules/tools/infrastructure/sensitive.rs:10`-`40`
- **Problem:** The default sensitive globs are hardcoded in `DEFAULT_SENSITIVE_PATTERNS` (sensitive.rs) and **re-typed verbatim** as prose inside `SYSTEM_PROMPT` (config.rs). When a user sets `KIRI_SENSITIVE_PATTERNS`, the enforced list changes but the prompt still advertises the defaults, so the model is told an inaccurate policy. Pure documentation/behavioral drift (the enforcement is correct regardless), but two copies of the same list will diverge.
- **Evidence:**
```
// config.rs SYSTEM_PROMPT
"Sensitive names: .env*, id_rsa, id_dsa, id_ecdsa, id_ed25519, *.pem, *.key, *.crt, *.p12, *.pfx, *.keystore, *.jks, credentials*, secrets*, .netrc, ...
```
- **Recommendation:** Either generate the prompt fragment from the active matcher at boot, or replace the verbatim list with a generic sentence ("the harness enforces a configurable sensitive-file list; a call against one is refused") so there is one source of truth.

### [SEC-07] First-run env-key import persists silently with no path confinement note
- **Severity:** Low
- **Category:** security
- **Location:** `src/app.rs:403`-`421`, `src/modules/provider/infrastructure/factory.rs:120`-`135`
- **Problem:** `resolve_credential` reads a provider key from a legacy env var (`NVIDIA_API_KEY` / `KIRI_<ID>_API_KEY`) and **writes it into the credential store** on first use, announcing only "imported API key for provider '…' into the credential store" on stderr. This is intended migration behavior and never logs the key, but it silently promotes a transient env secret to durable on-disk/keyring storage — a user exporting a key for a one-off CI run may not expect it to be persisted to `~/.kiri/credentials.json`. The env candidates are also filtered for blank values (good), and the generic-before-vendor ordering is correct.
- **Evidence:**
```rust
match secrets.set(&profile.id, &credential) {
    Ok(()) => eprintln!("kiri: imported API key for provider '{}' into the credential store", profile.id),
    ...
}
```
- **Recommendation:** Keep the behavior (it is a deliberate convenience), but make the persistence opt-out-able or at least state in the notice that the key was written to durable storage and where, so an operator can choose not to persist a CI secret. Defer as `security-debt` rather than block.

## Strengths
- **`Secret` is exemplary:** `Zeroizing<String>` inner, `Debug` redacts to `Secret(***)`, `Serialize` only ever feeds the keyring/0600 file, and a unit test locks the redaction (`provider.rs:194`-`239`). `Credential`/`OauthTokens` inherit the redaction through composition, and `ProviderProfile` (the TOML-serialized type) deliberately holds no secret — the "no secret in TOML" invariant holds end-to-end.
- **Untrusted-config containment is tight and tested:** `resolve_layers` discards everything from the workspace layer except `effort` (`config.rs:298`-`301`), and the sync trust-gate `risky_config_changes` flags base_url redirects, auth-downgrade-to-none, active-provider redirects, embeddings redirects, and sandbox weakening — each with a regression test (`sync_service.rs:292`-`365`).
- **I/O is uniformly bounded:** provider client has `connect_timeout`+`read_timeout` (`app.rs:53`-`56`, `config.rs:131`-`138`), `exec::run` enforces a kill-on-drop timeout and 64 KiB output cap, `git_cli` has a 120 s timeout, the SSE accumulator caps streamed bytes at 8 MiB (`openai/sse.rs:17`, `55`-`60`), and `run_command` clamps its timeout to `[1s, 10min]`. SSE/JSON parsing maps every `from_str` to a Result (no panic on attacker data) and falls back to `"{}"` for un-recoverable tool arguments. No `unsafe` (crate forbids it), and no runtime-reachable `unwrap/expect/panic`.
