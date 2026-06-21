# ADR 0002 — Tool calling with file CRUD behind a filesystem sandbox

- Status: Accepted
- Date: 2026-06-20

## Context

The chat client (ADR 0001) only exchanges text. To let the model act on the local filesystem we add
OpenAI-compatible **tool calling** (function calling), starting with a file CRUD tool set:
`read_file`, `write_file`, `edit_file`, `delete_file`, `list_dir`, `search`.

Once the model can write and delete files, the central risk is data loss outside the intended working
area. We need a confinement boundary so a tool can never touch a path the user did not authorize, plus a
human checkpoint before irreversible operations.

The provider streams responses (SSE). OpenAI-compatible `tool_calls` arrive fragmented across deltas
(keyed by `index`; `function.arguments` is concatenated), terminated by `finish_reason: "tool_calls"`.
A tool turn therefore requires accumulating fragments and a multi-step agentic loop (call → execute →
feed results back → continue), which the current single-turn REPL does not have.

## Decision

**Two-layer safety model.**

- **Layer 0 — in-process path confinement (`src/services/sandbox.rs`).** A `Sandbox` holds a
  canonicalized root and is the single chokepoint: every tool resolves its path through it and no tool
  calls `std::fs` with a raw path. Resolution rejects `..` and absolute paths lexically, then
  `canonicalize`s (resolving symlinks) and asserts the real path stays under the root. Creation resolves
  the deepest existing ancestor and confirms it is within root before appending the remaining
  components.
- **Layer 1 — human confirmation.** Before executing a destructive operation — delete, overwrite (write
  onto an existing file), edit (find/replace mutates existing content), and creating intermediate
  directories — the CLI prompts `[s/N]`. A decline returns a "declined by user" result to the model
  rather than executing. Create-in-existing-dir, read, list, and search run without a prompt.

**Sandbox root** is the clap flag `--path <dir>` (also `T_CLI_PATH`), defaulting to the current working
directory, canonicalized at boot with fail-fast on a missing or non-directory path. The flag uses clap's
derive `env`, consistent with the established CLI convention.

**Errors as data.** Tool failures (bad arguments, path escapes, missing files) are returned to the model
as result strings, never as panics or `Err` that abort the turn — the model reads the error and recovers.

**Bounded agentic loop** in `main.rs`, capped at 10 tool iterations per user turn to bound runaway loops.

**No new dependencies.** `search` is substring matching with manual recursion (no `regex`/`walkdir`).

## Consequences

- New modules: `src/models/tools.rs` (tool definition + call types), `src/services/sandbox.rs`,
  `src/services/tools.rs`. `src/models/chat.rs` evolves (`Role::Tool`, `Message.content: Option<String>`
  plus `tool_calls`/`tool_call_id`, `ChatRequest.tools`, streaming `tool_calls` fragments +
  `finish_reason`); `src/services/chat.rs` accumulates fragments and returns a `CompletedTurn`. The
  one-way dependency rule (`main → services → models`) is unchanged; within `models`, `chat` depends on
  `tools`.
- **Deferred to a future OS-level sandbox (Landlock on Linux / Seatbelt on macOS), tracked as
  `security-debt`:** Layer 0 does not close **TOCTOU** (a symlink swapped between check and use) nor
  **hard-link** aliasing (a hard link inside root to an inode also named outside root). For a
  single-user local CLI where the user already controls the filesystem, the residual risk is low and the
  confirmation layer gates the irreversible cases; OS-level enforcement is the correct long-term fix.
- Creating intermediate directories is allowed by default but always behind a confirmation that names
  the directories to be created, so the user retains per-operation control without a hard block.

## Update — 2026-06-20

Two changes after the initial implementation, keeping the decisions above otherwise intact.

**Directory CRUD + move.** The original set covered files only. To give files *and* directories full
CRUD plus relocation, three tools were **added** (existing tools are unchanged):

- `move_path` — move or rename a file **or** directory (`fs::rename`); resolves source through
  `resolve_existing` and destination through `resolve_create`, so both ends pass the sandbox chokepoint.
- `create_dir` — create a directory and any missing parents (the directory "create").
- `delete_dir` — delete a directory and its contents recursively (the directory "delete").

`delete_file` stays files-only; `list_dir` is the directory "read"; renaming via `move_path` is the
directory "update". `move_path` and `delete_dir` refuse to operate on the sandbox root itself.

Confirmation policy for the new tools: `delete_dir` always confirms with an explicit *recursive* prompt;
`move_path` confirms on overwrite or when it must create destination directories (mirroring `write_file`);
`create_dir` needs no confirmation — it is an explicit, non-destructive request (the earlier
"always confirm directory creation" rule targets *incidental* creation as a side effect of a write/move,
not a direct `create_dir` call).

**No fixed iteration cap.** The "bounded agentic loop capped at 10 tool iterations" is replaced by an
unbounded loop that runs until the model stops requesting tools, guarded by a **wall-clock checkpoint**
(`TOOL_CHECKPOINT`, 30 min): when a single user turn's tool loop exceeds the budget the CLI prompts the
user to continue (`[s/N]`); confirming resets the timer, declining ends the turn. This removes the
arbitrary count while keeping a human guard against an unattended runaway — now time-based, not
count-based.

## Update — 2026-06-20 — per-call approval, default-accept, system prompt

Refines Layer 1 and adds an assistant system prompt; supersedes the confirmation-scope notes above.

**Confirmation now gates every tool call.** Layer 1 no longer prompts only on destructive operations:
`confirmation_prompt` returns a prompt for **every** tool whose arguments parse — reads
(`read_file`/`list_dir`/`search`), plain creates (`write_file` of a new file, `create_dir`), and clean
moves included. Only unparseable arguments return `None` (and `execute` reports the error). This replaces
the earlier "read/list/search and create-in-existing-dir run without a prompt" and "`create_dir` needs no
confirmation" rules. The model is told (system prompt) that each call is user-approved and may be declined.

**Default is accept (`[S/n]`).** All prompts (tool confirmations and the time checkpoint) end in `[S/n]`
and default to acceptance: `accepted()` treats Enter/empty and any unrecognized input as yes; only an
explicit `n`/`nao`/`não`/`no` declines. Pressing Enter approves, matching the Claude Code flow.

**System prompt.** A `SYSTEM_PROMPT` const is seeded once as the first (`Role::System`) message of the
session (`Message::system`), persisting across turns via `history.clone()`. It sets the assistant's
identity (a coding agent acting on the workspace through the tools — not a demo/test narrator), tells it to
state a short plan then act and narrate each action coherently, ask when a request is ambiguous, stay
grounded (read before asserting; report failures honestly), write senior-level human-readable code valuing
quality over quantity, and reply in the user's language while keeping code/identifiers/file contents in
English.

## Update — 2026-06-21 — movable workspace and out-of-root access

**Layer 0 relaxed: confinement applies to relative paths against a movable active root.** The hard
"everything stays under one root" rule is replaced by:

- **Relative paths** still resolve under the active workspace root, still reject `..`, and are still
  asserted within the root (symlink escapes via a relative path remain blocked).
- **Absolute paths and `~/…`** are now allowed to resolve *outside* the active root. `Sandbox::resolve_*`
  branch on the (tilde-expanded) path: absolute → `canonicalize`/ancestor-resolve without `assert_within`;
  relative → unchanged. `search` bounds its recursion and relative display to the resolved start directory
  rather than the root.
- **The active root is movable** at runtime via the `/cd <dir>` REPL command (`Sandbox::new` re-validates);
  the model does not change it — it reaches outside via an absolute path the user approves.

**Confirmation default depends on location.** `confirmation_prompt` returns a `Confirmation { prompt,
default_accept }`. In-workspace (relative) operations keep `[S/n]` (default accept); operations on an
explicit absolute/`~` path default to **decline** `[s/N]`, requiring a deliberate "yes" — the guard that
makes relaxing Layer 0 acceptable. `answer_approves(answer, default_accept)` interprets the reply.

**Security-debt.** With Layer 0 no longer a hard boundary, the model can reach any path the user can, gated
only by Layer 1 (and default-decline outside the workspace). Tracked as `security-debt`: prefer an
OS-level sandbox (Seatbelt/Landlock) and/or an explicit allowlist of reachable roots; never default-accept
an out-of-workspace operation; consider warning when a path resolves into sensitive locations (e.g.
`~/.ssh`, `~/.aws`).
