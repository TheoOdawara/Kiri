# ADR 0015 — Portable profile sync via a private git repo

- Status: Accepted
- Date: 2026-06-26

## Context

`~/.kiri` (global config, secrets, shared memory, sessions) is machine-local: only a project's `.kiri/`
layer travels via the project's own git. A user working across machines had to re-enter providers and
lost the cross-machine accumulation of learned memory (ADR 0013). The goal: "work the same on any
computer" — without standing up a backend, and without ever moving secrets off a machine.

## Decision

### What syncs, and what never does

A dedicated git work-tree at `~/.kiri/sync` holds the **portable profile**: the non-secret `config.toml`
and the shared memory exported as **deterministic NDJSON** (one entry per line, sorted by id; embedding
vectors excluded — they are machine-local derived data). It **never** holds `credentials.json`, the OS
keyring, embedding caches, or `*.db` binaries. The whitelist (only config + NDJSON are written into the
tree) is the airtight guarantee; a `.gitignore` is the backup. Sessions remain machine-local for now
(privacy default; forward-compatible).

### Mechanism

Shell out to the system `git` (a `Git` port + `GitCli` adapter, mirroring `exec`'s spawn +
`kill_on_drop` + timeout), so the user's own credential helper / SSH authenticates to the remote — Kiri
never handles repo credentials. The work-tree is separate from `~/.kiri` itself, so the secret file is
never even inside the tree. The profile lives on a pinned `main` branch so push/pull agree across hosts.

- `init <url>` — set up the work-tree and remote.
- `push` — export config + memory, commit, push.
- `pull` — `fetch` + `reset --hard` the work-tree (it holds only export artifacts), then **merge memory
  last-write-wins by `updated_at`** into the live database (the DB is outside the tree, so local memory
  survives and merges), and apply config under the trust check.
- `status` — the work-tree's git status.

Memory merge is last-write-wins per entry id, making a re-pull idempotent. It is never a destructive DB
clobber — the binary `*.db` is never synced; only the text NDJSON is.

### CLI + headless route

`kiri sync init|push|pull|status` is a clap subcommand. `main` parses the CLI and dispatches the sync
route **before** `app::wire` (which requires a TTY), so sync works over SSH and in scripts. A `/sync` TUI
command runs a push live.

### Security — trust-on-pull

The real risk is not secret leakage (the whitelist prevents it) but a **pulled `config.toml` becoming the
trusted global layer** (ADR 0012): a changed provider `base_url` could redirect a stored credential, or a
weakened sandbox could lower confinement. So `pull` diffs the incoming config and, on a risky change,
**refuses to apply the config** unless `--force` — printing exactly what it skipped. Memory still merges
(it carries no such authority). Residual, documented: memory content syncs as plaintext to the user's own
private repo.

The trust gate reasons over a **typed view** of the security-relevant fields, parsed against the kernel
primitives `AuthMethod` / `SandboxMode` / `NetworkStance` (no hand-typed `"none"`/`"off"`/`"allow"` magic
strings that drift from the loader on a variant rename). Sandbox modes are **ranked** `require > os > off`,
so the gate flags *any* strictly-lower incoming rank — `require → os`, `require → off`, `os → off` — not
only the extreme `→ off`; strengthening (`os → require`) and an absent-to-baseline `os` are never flagged.
The kernel placement keeps a future move of the gate into `sync/domain` pure (no `domain → infra`/`tools`).

## Consequences

- A new machine becomes productive with `kiri sync init <url> && kiri sync pull`: same providers (minus
  the secret, re-entered or env-imported once) and the full accumulated memory.
- No backend, no new service to run; the user owns the repo and its access.
- Conflict handling is last-write-wins, not a 3-way merge — sufficient for a single user across machines;
  richer merge is future work if multi-user sharing is ever wanted.
