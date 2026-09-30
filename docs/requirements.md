# Kiri — Requirements

Living source of product truth. A feature's depth lives in its `/spec`; this doc holds the breadth and
links to it.

## Problem

AI coding agents optimize for speed over control: edits scroll past, "done" is declared on red builds,
and each tool locks you to one vendor. Kiri is a terminal coding agent for engineers who want the
opposite:

- **Full control** — every reasoning step, tool call, and diff is visible; nothing with side effects
  runs without the user's policy allowing it.
- **Any model** — the user brings whatever provider they have: API key, local model, or a
  subscription (ChatGPT Plus/Pro, Claude Pro/Max).
- **Built-in gate** — the agent cannot declare work finished while the repository's own checks are red.

## Users and stakeholders

- **Primary user and sole decision-maker:** the author.
- **Secondary users:** developers who install it from the open-source release. Onboarding and docs must
  work for someone who has never seen the repo.

## Brand (preserved from the previous codebase — the only thing preserved)

Everything else from the previous implementation is discarded and is not a reference for any decision.

- **Assets:** `docs/marca/` — `Logo.png`, `icon.png`, `Design.png`, `branding.md`.
- **Name and concept:** Kiri (桐), the paulownia — the family *kamon*, the *kiri-dansu* chest that guards
  what is precious, and a homophone of 切り ("to cut"). The mark is the **Kiri-Gate**: the *go-shichi-no-kiri*
  crest inside a *tsuba* read as a containment ring — the Quality Gate — around three foundations:
  **Code · Infra · Data**. Monochrome, technical, forged.
- **Tagline:** *Engineering-Grade Code Harness — Forged from Tradition, Built for Precision.*
- **Seal:** the Kiri-Gate drawn in block characters, centered on launch with the tagline beneath it
  (`docs/marca/seal.txt`).
- **Motion concept:** forging and cooling — content forges in, cools to steel, the gate tempers when work
  settles. The concept is kept; how it plays is redesigned.
- **Palette:** "Tamahagane Void" (`docs/marca/branding.md`) is the starting point, not a constraint. The
  redesign keeps the steel-and-gate character but reads lighter and easier than the previous near-black.
- **Open for redesign:** glyphs, screen layout, and everything else of the previous interface.

## Scope

### Milestone 1 — the core (usable daily)

- Agent loop with streaming reasoning, text, and tool calls.
- Providers by API key and local endpoint: any OpenAI-compatible endpoint, Anthropic and
  Anthropic-compatible, OpenAI, local (Ollama / LM Studio).
- Built-in tools: read, search, edit/write files, run shell commands.
- Approval with modes: plan (read-only), default (asks before side effects), auto.
- Inline interactive interface.
- Brand applied: seal, palette, tagline.

### Later waves (order decided as development proceeds)

- Full-screen interactive interface.
- Built-in gate with self-correction.
- OS sandbox on all three platforms.
- Auto-mode safety classifier.
- Subscriptions: "Sign in with ChatGPT"; Claude Pro/Max.
- Extensions: MCP, skills/commands (with the bundled skills), hooks, subagents.
- Continuity: resumable sessions, checkpoints/rewind, persistent memory, sync across machines.
- Headless mode (`kiri -p`) and IDE integration via ACP.
- Distribution: install script, package managers, auto-update.

### Out of scope

- Running Windows through WSL. Windows is native.
- GUI, desktop app, or web UI.
- Hosted/cloud agent execution.
- Team or multi-user features, accounts, billing.

## Functional requirements

### Agent

- Runs a multi-turn loop: the model reasons, calls tools, sees results, and continues until it answers.
- Streams reasoning, text, and tool calls live, as they arrive.
- The user can interrupt a turn at any moment without losing the conversation.
- Reads the project's instruction file(s) into context at start.

### Providers

- Supported by API key or local endpoint: OpenAI-compatible (any base URL), Anthropic and
  Anthropic-compatible (any base URL — MiniMax, GLM, Kimi and similar), OpenAI, local (Ollama /
  LM Studio, no key).
- Subscriptions: ChatGPT Plus/Pro through the same "Sign in with ChatGPT" OAuth flow other coding agents
  offer; Claude Pro/Max by driving the user's installed, logged-in official `claude` CLI
  ([ADR 0001](decisions/0001-architecture.md)).
- Claude Pro/Max is opt-in: before enabling it, the user is warned that Anthropic restricts subscription
  use outside its own apps and may enforce that against their account, and confirms.
- Provider, model, and reasoning effort are switchable live, inside a session.
- Adding a provider is done from within Kiri; no manual file editing required.

### Tools

- Read, search, edit, and write files; run shell commands.
- Every edit is shown as a diff.
- Tool output returned by the environment is data, never instructions to Kiri.

### Safety and control

- Modes: **plan** (read-only), **default** (asks before any side effect), **auto** (acts without asking,
  within the sandbox, the permissions, and the classifier when one is configured).
- Permissions are configurable globally and per project: which actions run, ask, or are blocked.
- In auto mode, an optional classifier decides per action: run, ask the user, or block. The user picks
  the model — their active provider's model or any other configured one. Reference behavior: Jev
  (TypeSafe AI), which scores each call with session context and flags prompt injection in tool results.
- Without a classifier, auto mode still runs, bounded by the sandbox, the permissions, and the user's
  global/project workflow (rules, hooks).
- Commands run inside OS-level confinement on Windows (native mechanism — no WSL, no VM), Linux, and
  macOS.
- Configuration or extensions supplied by an untrusted repository can never loosen safety, redirect
  credentials, or run code without explicit user trust.

### Built-in gate

- Discovers the repository's own checks (format, lint, typecheck, build, test).
- Before the agent declares work finished, the gate runs; on failure the agent receives the failure and
  keeps working until the gate passes or its attempt budget is spent, then hands control to the user
  with the failure.

### Interfaces and modes of use

- **Inline** interactive interface: the conversation lives in the terminal's native scrollback.
- **Full-screen** interactive interface: alternate screen, full layout control.
- The user chooses the interface.
- **Headless** (`kiri -p`): one prompt, text or JSON output, usable from scripts and CI.
- **IDE integration** via ACP (Agent Client Protocol).

### Extensions

- **MCP** servers exposed as tools.
- **Skills/commands**: Markdown files turned into reusable instructions and commands.
- **Hooks**: user scripts triggered on agent events.
- **Subagents**: the agent dispatches isolated subagents for scoped tasks.
- Each exists at a global (user) level and a project level.
- **Bundled skills** ship built in, with no setup: `ponytail` (DietrichGebert/ponytail), `humanizer`
  (blader/humanizer), `i-have-adhd` (ayghri/i-have-adhd), and `grill-me` (mattpocock/skills).
- Bundled and third-party skills stay current: when their upstream source publishes an update, Kiri
  receives it.

### Continuity

- **Sessions**: list and resume past conversations per project. Closing the terminal does not end a
  running session; the user reattaches to it.
- **Checkpoints/rewind**: roll the agent's file edits back to a point in the conversation.
- **Persistent memory**: the agent keeps facts and preferences across sessions.
- **Sync**: configuration, memory, and extensions — all under `~/.kiri` — synced across the user's
  machines through a private GitHub repository.
- First-run setup offers to create that repository, using `gh` when it is installed. The user can
  decline, and Kiri works fully without sync; it can be turned on later.

### Distribution

- `cargo install`.
- Prebuilt binaries on GitHub Releases for the three platforms, plus a one-line install script
  (`curl … | sh` on Linux/macOS, PowerShell equivalent on Windows) that fetches them.
- Package managers: winget/scoop, Homebrew, Linux packages.
- Auto-update.

## Non-functional requirements

- **Language:** Rust.
- **Platforms:** Windows, Linux, macOS — all native, first-class, same feature set. No emulation layer.
- **Architecture proportional to the problem:** one engine serves four front-ends (inline, full-screen,
  headless, ACP), so the engine knows no UI. No layer, crate, or abstraction exists without a concrete
  consumer. Decided in [ADR 0001](decisions/0001-architecture.md).
- **Defined before built:** every capability has a closed `/spec` before code.
- **Stability:** no crash on a runtime path; every network and process call has a timeout; every failure
  reaches the user with a cause, never silence.
- **Responsiveness:** the interface never blocks on the network or a tool; input stays live while the
  model streams.
- **Secrets:** API keys and tokens never appear in config files, logs, or transcripts.
- **Terminal:** degrades cleanly without truecolor; honors `NO_COLOR`.
- **License:** MIT.

## Acceptance criteria

- Given a configured API key, when the user sends a prompt, then reasoning, text, and tool calls stream
  live and the turn ends with an answer.
- Given a running turn, when the user interrupts, then it stops and the conversation stays intact.
- Given a session on provider A, when the user switches to provider B, then the next turn uses B with the
  history preserved.
- Given only a local Ollama or LM Studio server, when the user selects it, then Kiri works with no key.
- Given plan mode, when the model attempts an edit or a side-effecting command, then it is refused.
- Given default mode, when the model proposes a side effect, then nothing runs until the user approves.
- Given auto mode, when the classifier judges an action unsafe, then the action is blocked or escalated
  to the user.
- Given auto mode with no classifier configured, when the model proposes an action, then the sandbox and
  the configured permissions alone decide whether it runs, asks, or is blocked.
- Given the sandbox, when a command tries to write outside the allowed area, then the OS stops it — on
  all three platforms.
- Given a repository whose config tries to loosen safety or redirect a credential, when Kiri loads it,
  then the attempt is ignored and reported.
- Given red checks, when the agent tries to finish, then it receives the failure and continues; when the
  attempt budget is spent, it stops and hands the failure to the user.
- Given the inline interface, when the session runs, then the transcript stays in the terminal's native
  scrollback; given full-screen, then it runs in the alternate screen.
- Given `kiri -p "<prompt>"`, when it runs, then it prints the result without an interactive UI and exits
  non-zero on failure.
- Given an ACP-capable IDE, when it connects to Kiri, then the user drives the agent from the IDE.
- Given a trusted MCP server, a skill, a hook, or a subagent definition, when Kiri starts, then it is
  usable in the session.
- Given a fresh install, when the first session starts, then every bundled skill is available.
- Given a skill whose upstream publishes a new version, when Kiri updates, then the new version is used.
- Given a past session, when the user resumes it, then the conversation continues where it stopped.
- Given agent edits after a checkpoint, when the user rewinds, then the files return to that point.
- Given a fact stored in memory, when a later session needs it, then the agent recalls it.
- Given two machines with sync on, when memory or extensions change on one, then the other receives it.
- Given a first run, when the user declines creating the sync repository, then Kiri works normally and
  sync can be enabled later.
- Given a ChatGPT Plus/Pro account, when the user signs in with ChatGPT, then turns draw on that plan.
- Given a logged-in official `claude` CLI, when the user enables Claude Pro/Max, then Kiri shows the
  account-risk warning, and only after confirmation turns run through the CLI with Kiri's tools.
- Given a running session, when the user closes the terminal and opens Kiri again, then the session is
  still running and the user reattaches to it.
- Given an Anthropic-compatible endpoint (e.g. MiniMax), when the user sets its base URL and key, then
  Kiri works with it.
- Given a new release, when auto-update runs, then Kiri replaces itself with the new version.

## Open questions

None open. Architecture → [ADR 0001](decisions/0001-architecture.md).

## Evolution log

- **2026-09-30** — Kickoff after a full reset. The previous implementation is discarded; only the brand
  (assets, concept, palette, seal, tagline) is preserved. Scope captured from the author's interview:
  terminal coding agent, Rust, native on three platforms, open source, core-first milestones. Verified
  that Anthropic has blocked third-party use of Claude Pro/Max OAuth since 2026-04-04 and that OpenAI
  officially supports "Sign in with ChatGPT" for third-party tools; impersonating a vendor client moved to
  Out of scope.
- **2026-09-30** — Claude Pro/Max subscription moved back into scope by the author's decision, as an
  opt-in with an explicit ban-risk warning; the mechanism became an open question. Anthropic-compatible
  endpoints (MiniMax, GLM, Kimi) added as a provider kind.
- **2026-09-30** — Author settled five open questions: Milestone 1 ships the inline interface; Claude
  Pro/Max goes through Kiri's own proxy; ChatGPT uses the same OAuth flow other agents offer; Windows gets
  a native sandbox; wave order is decided as development proceeds. Added: bundled `ponytail` and
  `humanize` skills that track their upstream; a free, low-end-hardware classifier as a constraint; sync
  data lives in `~/.kiri`.
- **2026-09-30** — Bundled skills set to `ponytail`, `humanizer`, `i-have-adhd`, `grill-me`, with their
  upstream repos. Jev recorded as the classifier's reference behavior; it stays out as the implementation
  because it is paid and remote.
- **2026-09-30** — Classifier made optional and user-chosen (any configured model); without one, auto
  mode rests on the sandbox, configurable permissions, and the user's workflow. The free/local-hardware
  constraint was dropped. Sync goes through a private GitHub repo offered at first run (via `gh`),
  declinable. `grill-me` upstream set to mattpocock/skills.
- **2026-09-30** — Architecture decided in ADR 0001 (daemon owning every session, ACP boundary, three
  crates, capability modules, data-only extensions). Claude Pro/Max moved from an own proxy to driving the
  official `claude` CLI, after research showed impersonation breaks on every Claude Code release while
  `claude -p` remains the tolerated path; the warning was reworded accordingly. Sessions now survive a
  closed terminal.
- **2026-09-30** — Brand scope narrowed by the author: logo, block-character seal, and tagline are kept
  as they are; the forging/cooling motion is kept as a concept to be redesigned; the palette is a starting
  point to be made lighter and more readable; glyphs and layout are open.
