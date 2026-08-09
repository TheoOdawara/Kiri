# Kiri-V2 domain language

This glossary records terms that are part of the current Kiri-V2 product
contract. It contains domain language, not implementation structure.

## Session

A live unit of work started from a user request. A session can run, wait for a
question, receive queued messages, invoke skills or agents, and produce a
completion report.

## Mode

The session's current interaction and execution policy, such as `Plan`,
`Default`, or `Auto`. The user can change the mode during an active session.

## TUI

Kiri's primary interactive terminal interface. It is the canonical surface for
supervising and steering live work.

## CLI

Kiri's first-class command interface for automation, headless use, scripts, and
direct actions. It follows the same session, policy, security, and execution
contracts as the TUI. A session started through the CLI remains visible and
controllable in the TUI when the user is authorized.

## Windows-native execution

The ability to use authorized commands and tools installed on the Windows host.
It is a first-class Kiri-V2 capability and is subject to the same policy and
security contracts as other execution.

## PowerShell 7

Kiri's canonical Windows shell for scripts and compound shell commands. It is
invoked as `pwsh`; direct structured program execution remains separate from
shell execution. The Kiri installer installs it as a required Windows
dependency.

## Credential

A secret value referenced by a provider or another Kiri integration. Raw
credential values are not configuration or workflow content.

## Credential backend

The storage mechanism used by Kiri for credentials. The file backend is the
default; the native OS keyring is optional and switchable, with verified
migration between backends.

## Distribution channel

A supported package-manager path through which a user installs and updates
Kiri, such as npm, Bun, Homebrew, or AUR.

## Native package artifact

The prebuilt Kiri executable for one supported operating system and CPU target.
Distribution channels deliver this artifact rather than compiling Kiri during
installation.

## Decision question

A question with alternatives that asks the user to choose how the work should
proceed. It is distinct from an acceptance or permission prompt for a specific
action.

## Activity stream

The user-facing progress shown while the model works, including activities
such as reads and runs. The stream remains visible even when expanded thinking
is collapsed.

## Queued message

A message sent by the user while the model is busy. It waits in the session's
input queue until the first safe point at which it can be delivered. Priority
and fairness among multiple queued messages are not yet defined.
