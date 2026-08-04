# 0008. TUI-first with a first-class CLI

- Status: accepted
- Date: 2026-08-04

## Context

Kiri's main human workflow is interactive: the user starts Kiri in a working
directory, observes the session, answers questions, changes modes, and steers
the work while it runs. That workflow needs a full-screen terminal interface.

Kiri also needs to operate from scripts, automation, headless environments,
and direct commands. Treating the CLI as only a launcher would make useful
actions such as invoking an agent unnecessarily dependent on the TUI.

## Decision

Kiri is TUI-first:

- Running `kiri` in the working directory opens the primary interactive TUI.
- The TUI is the canonical experience for supervising and steering live work.
- The CLI is a first-class interface for automation, scripts, headless use,
  and direct actions such as invoking an agent.
- CLI and TUI use the same Kiri session, policy, security, and execution
  contracts. The CLI does not bypass the policy engine or create a weaker
  execution path.
- A session started through the CLI, including a session that invokes child
  agents, remains visible in the TUI. The user can inspect, follow, attach to,
  and control it there when authorized.
- The exact CLI commands, flags, output formats, exit codes, and non-interactive
  behavior remain open in the CLI and TUI decisions.

TUI-first describes the primary interactive experience; it does not mean that
the CLI is secondary in capability or limited to startup and diagnostics.

## Consequences

- A human running `kiri` receives the full interactive screen by default.
- Automation can use Kiri without driving terminal UI input.
- Agents, skills, sessions, and other direct actions can be exposed through
  CLI commands when their contracts are defined.
- CLI-started sessions are not hidden or detached from the TUI by default.
- The product must keep TUI and CLI behavior aligned without duplicating policy
  or execution semantics.

## Alternatives considered

- Making the CLI only a TUI launcher was rejected because it blocks automation
  and direct actions behind an interactive interface.
- Making the CLI-first workflow canonical was rejected because the primary
  Kiri experience requires continuous visual supervision and steering.
- Giving CLI operations a separate policy path was rejected because it would
  create a security and behavior gap between interfaces.
