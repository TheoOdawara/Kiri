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
