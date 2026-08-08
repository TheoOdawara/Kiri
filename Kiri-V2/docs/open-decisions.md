# Kiri-V2 open decisions

- Status: working register
- Date: 2026-08-08

This document tracks design questions that still need an explicit decision.
An unchecked item is not an implementation task by itself. It becomes a
contract only after we explicitly decide it together and record it under
`Kiri-V2`. Document status describes lifecycle and does not invalidate a
decision we already made.

## Authority boundary

Every decision that we explicitly made together and recorded under `Kiri-V2`
is valid and current. The architecture statements currently present in the
repository's `AGENTS.md` are not treated as settled Kiri-V2 decisions. They
must not close any item in this register unless we also record them under
`Kiri-V2`.

The current records are:

- ADRs 0001–0013 are recorded as accepted decisions.
- Specification 0001 is recorded as proposed, but the decisions made in it
  are valid; `proposed` describes document lifecycle, not decision validity.
- Historical code, Claude/Codex conventions, and external examples are
  references only.

## Current valid baseline

The following Kiri-V2 records are the current design baseline:

- [ADR 0001: Foundation stack](decisions/0001-foundation-stack.md)
- [ADR 0002: Harness configuration and discovery](decisions/0002-harness-configuration-and-discovery.md)
- [ADR 0003: Execution modes and policy enforcement](decisions/0003-execution-modes-and-policy-enforcement.md)
- [ADR 0004: Memory architecture and backends](decisions/0004-memory-architecture-and-backends.md)
- [ADR 0005: Live session control](decisions/0005-live-session-control.md)
- [ADR 0006: Primary user journey](decisions/0006-primary-user-journey.md)
- [ADR 0007: Complete Kiri-V2 v1 scope](decisions/0007-complete-v1-scope.md)
- [ADR 0008: TUI-first with a first-class CLI](decisions/0008-tui-first-cli-first-class.md)
- [ADR 0009: Windows-native execution is first-class](decisions/0009-windows-native-execution.md)
- [ADR 0010: Native Windows runtime and PowerShell 7](decisions/0010-native-windows-runtime-and-pwsh.md)
- [ADR 0011: Installer-managed PowerShell 7 prerequisite](decisions/0011-installer-managed-powershell.md)
- [ADR 0012: Package distribution and channel-owned auto-update](decisions/0012-package-distribution-and-auto-update.md)
- [ADR 0013: MSIX-managed PowerShell 7 prerequisite](decisions/0013-msix-managed-powershell.md)
- [Specification 0001: On-disk schemas](specs/0001-on-disk-schemas.md)

The checklist below contains only decisions that are not already defined in
those records. Reopening a baseline decision requires an explicit replacement
decision in a new or updated Kiri-V2 record.

## 1. Product boundary and v1 scope

- [x] Define Kiri's primary user workflow and the first complete user journey.
      See [ADR 0006: Primary user journey](decisions/0006-primary-user-journey.md).
- [x] Define the v1 scope and the explicit non-goals for the first usable release.
      See [ADR 0007: Complete Kiri-V2 v1 scope](decisions/0007-complete-v1-scope.md).
- [x] Decide whether Kiri is TUI-first, CLI-first, or must support both equally.
      See [ADR 0008: TUI-first with a first-class CLI](decisions/0008-tui-first-cli-first-class.md).
- [x] Define the supported operating systems, distribution targets, and runtime
      model for each operating system. Linux, macOS, and Windows use native
      runtimes. The package channels, CPU targets, and minimum operating-system
      policy are defined in [ADR 0012](decisions/0012-package-distribution-and-auto-update.md).
- [x] Decide whether a Windows runtime is native, WSL-backed, or another model.
      Windows-native is decided; WSL is not the required Windows runtime in v1.
      See [ADR 0010](decisions/0010-native-windows-runtime-and-pwsh.md). The
      native sandbox implementation remains open.
- [x] Define the required PowerShell 7 version and installation or bundling policy.
      The installer installs the latest stable release available at installation
      time as an MSIX or MSIXBundle. See [ADR 0013](decisions/0013-msix-managed-powershell.md).
- [x] Define the behavior when PowerShell 7 installation or verification fails.
      The installer retries internally; after the retry fails, it warns the
      user and explains how to resolve the dependency. See [ADR 0011](decisions/0011-installer-managed-powershell.md).
- [x] Define the retry count and timing for PowerShell 7 installation and
      verification. There are three total attempts: initial, after 1 second,
      and after 3 seconds. See [ADR 0011](decisions/0011-installer-managed-powershell.md).
- [x] Define `pwsh` discovery after installation. Kiri checks `PATH` first and
      then checks the registered MSIX installation, with the standard MSI
      directory retained only for existing MSI installations. See [ADR 0013](decisions/0013-msix-managed-powershell.md).
- [x] Define elevation timing for PowerShell 7 installation. The installer
      requests elevation immediately when invoked. See [ADR 0011](decisions/0011-installer-managed-powershell.md).
- [x] Define the supported package distribution channels and automatic update
      ownership. See [ADR 0012](decisions/0012-package-distribution-and-auto-update.md).
- [x] Define the PowerShell installer mechanism. Windows npm and Bun packages
      use the official signed MSIX or MSIXBundle distribution, preferably via
      WinGet; the update or pinning policy after installation remains open. See
      [ADR 0013](decisions/0013-msix-managed-powershell.md).
- [ ] Define PowerShell 7 update or pinning behavior after installation.
- [ ] Decide whether local keyless providers are first-class in v1.
- [ ] Decide which remote providers, if any, are supported in v1.
- [ ] Decide whether Claude, Codex, or other harness compatibility means
      conventions only, import, migration, or no compatibility.

## 2. Architectural shape

- [ ] Choose the architectural style for Kiri-V2.
- [ ] Define the bounded contexts or modules and their ownership boundaries.
- [ ] Define dependency direction between domain, application, infrastructure,
      presentation, and process-control code.
- [ ] Decide whether ports and adapters are required everywhere or only at
      external and security-sensitive boundaries.
- [ ] Define the composition root and the allowed responsibilities of startup
      and boot files.
- [ ] Define process boundaries: one process, TUI plus supervisor, or another
      arrangement.
- [ ] Decide where session state, policy state, provider state, and memory
      state live in the architecture.
- [ ] Define the async, blocking, cancellation, and task-ownership model.
- [ ] Define the domain error model, diagnostics model, and user-facing error
      mapping.
- [ ] Decide the event model used between modules and processes.
- [ ] Decide whether `unsafe` is prohibited and record the reason if so.

## 3. Harness and discovery

- [ ] Define project-root discovery, including nested repositories and
      monorepos.
- [ ] Define how multiple workspace roots are represented.
- [ ] Define discovery behavior for symlinks, inaccessible directories, and
      malformed resource files.
- [ ] Define tie-breaking when resources collide by ID, kind, scope, or origin.
- [ ] Define how bundled resources are packaged, versioned, upgraded, and
      removed.
- [ ] Define cache invalidation and live reload behavior for changed resources.
- [ ] Define instruction size limits, ordering, conflict diagnostics, and
      behavior when the selected instruction file is invalid.
- [ ] Define whether resources can reference other resources by ID and how
      those references are resolved.
- [ ] Decide whether Kiri has a plugin/package format beyond discovered files.
- [ ] Define provenance, trust reset, and update behavior when executable
      resource content changes.

## 4. Configuration contract

- [ ] Define defaults for every optional configuration field.
- [ ] Define configuration migration rules between schema versions.
- [ ] Define behavior for missing global configuration and missing project
      configuration.
- [ ] Define the closed provider `kind` catalog and its adapter requirements.
- [ ] Define command catalog entries and their configuration surface.
- [ ] Define the exact credential reference model and where credential values
      are stored.
- [ ] Define whether configuration edits are watched, reloaded, or applied
      only at session boundaries.
- [ ] Define concurrent-edit, lock, backup, rollback, and interrupted-write
      behavior for configuration files.
- [ ] Define how environment variables may be referenced, if at all.
- [ ] Define the full filesystem-request approval and revocation lifecycle.

## 5. Workflow resources

- [ ] Define command-template argument syntax, quoting, interpolation, and
      escaping.
- [ ] Define command input/output contracts and exit-status handling.
- [ ] Define `requires` dependency edges, ordering, optional dependencies,
      cycles, and disabled dependencies.
- [ ] Define skill references, supporting assets, and loading boundaries.
- [ ] Define agent invocation, child-agent limits, recursion, model selection,
      and result delivery.
- [ ] Define hook stdin, environment, working directory, timeout, output,
      failure, retry, and cancellation behavior.
- [ ] Define validator resources and whether validators are built-in,
      discovered, or both.
- [ ] Define the MCP server/tool schema and its trust and policy boundary.
- [ ] Define resource update, deletion, stale override, and orphan cleanup
      behavior.
- [ ] Define whether resource bodies support templates, variables, includes,
      or plain Markdown only.

## 6. Providers and model interaction

- [ ] Define the provider capability model: streaming, tools, images,
      structured output, reasoning, and cancellation.
- [ ] Define the provider request and response boundaries without leaking
      provider DTOs into the domain.
- [ ] Define the streaming event contract and terminal-event rules.
- [ ] Define timeout, cancellation, retry, backoff, and partial-response
      behavior.
- [ ] Decide whether automatic retry is ever allowed and for which operations.
- [ ] Define model discovery, model selection, aliases, and unavailable-model
      behavior.
- [ ] Define token, usage, cost, quota, and provider-error reporting.
- [ ] Define provider health checks and whether they may access the network
      outside a running session.

## 7. Policy and security

- [ ] Define the typed action and effect vocabulary used by the policy engine.
- [ ] Define policy rule precedence, inheritance, and conflict resolution.
- [ ] Define the classifier input, output, confidence/uncertainty handling,
      and explainability shown to the user.
- [ ] Define the exact `allow`, `ask`, and `deny` decision contract.
- [ ] Define persistent approval identity, scope, expiry, revocation, and
      invalidation after command or resource changes.
- [ ] Define project trust identity, storage, expiry, revocation, and reset.
- [ ] Define hard-deny invariants that no user or project setting can change.
- [ ] Define redaction rules for files, environment, command output, provider
      output, logs, and audit events.
- [ ] Define audit event schema, retention, rotation, access, and deletion.
- [ ] Define failure behavior when policy evaluation, redaction, audit, or
      approval storage is unavailable.
- [ ] Define destructive-action confirmation and recovery expectations.

## 8. Sandbox and operating-system enforcement

- [ ] Define the supported sandbox implementation for each target operating
      system.
- [ ] Define whether the sandbox is mandatory for each execution mode.
- [ ] Define the exact authorized-root, path-normalization, symlink, junction,
      reparse-point, and time-of-check behavior.
- [ ] Define process-tree ownership, termination, orphan cleanup, and signal
      behavior.
- [ ] Define defaults and hard ceilings for time, output, temporary storage,
      memory, file descriptors, and child processes.
- [ ] Define network enforcement, DNS behavior, proxying, domain rules, and
      loopback access.
- [ ] Define missing-sandbox behavior per mode and per operating system.
- [ ] Define the credential broker boundary and subprocess inheritance rules.
- [ ] Define how sandbox, policy, and provider failures are surfaced to the
      user and preserved in audit data.

## 9. Live sessions and supervisor

- [ ] Define the supervisor lifecycle, startup ownership, shutdown, and
      single-instance behavior.
- [ ] Define the TUI-to-supervisor IPC protocol, transport, framing, and local
      authentication.
- [ ] Define the session state machine and legal transitions.
- [ ] Define the session event model, ordering, replay, and backpressure.
- [ ] Define the persistent session schema, storage engine, migrations, and
      corruption recovery.
- [ ] Define queue fairness, priority, cancellation, and concurrency defaults.
- [ ] Define exact pause, stop, detach, crash, and operating-system-restart
      semantics.
- [ ] Define resume reconciliation and what state is safe to replay.
- [ ] Define TUI screens, keybindings, focus behavior, terminal-size support,
      and accessibility requirements.
- [ ] Define what session data is visible to which user, project, or child
      agent.

## 10. Memory

- [ ] Define remaining runtime-owned paths not fixed by Specification 0001.
- [ ] Define note filenames, directory layout, index updates, and change-log
      format.
- [ ] Define memory extraction triggers and the boundary between suggestion,
      proposal, and write.
- [ ] Define the user experience for `disabled`, `silent`, and `explicit`
      memory policies.
- [ ] Define duplicate detection, conflict presentation, merge, replacement,
      and deletion behavior.
- [ ] Define backend migration locking, interruption recovery, and rollback.
- [ ] Define search, indexing, lazy loading, stale-note detection, and lint
      behavior.
- [ ] Define privacy, redaction, retention, export, and permanent deletion.
- [ ] Define concurrent writes and ownership when multiple harnesses use the
      cross-harness backend.
- [ ] Define how memory versioning interacts with project version control.

## 11. Persistence and operational data

- [ ] Inventory every durable data class: configuration, trust, approvals,
      audit, sessions, memory, caches, and provider metadata.
- [ ] Choose the storage format for each data class and define why it is
      separate or shared.
- [ ] Define schema versioning, migrations, atomicity, locking, and recovery
      for each durable store.
- [ ] Define permissions, encryption expectations, and secret exposure rules
      for local files and databases.
- [ ] Define backup, export, reset, uninstall, and data-retention behavior.
- [ ] Define which operational data is safe to expose in the TUI or model
      context.

## 12. CLI and TUI behavior

- [ ] Define the command-line surface, subcommands, flags, and exit codes.
- [ ] Define interactive versus non-interactive behavior and terminal absence.
- [ ] Define approval, plan, question, error, and cancellation UX.
- [ ] Define output formats for humans, scripts, and diagnostics.
- [ ] Define keyboard shortcuts, discoverability, focus, and minimum terminal
      dimensions.
- [ ] Define localization and the language of user-facing messages.
- [ ] Define update, version, help, and diagnostic commands.

## 13. Quality, compatibility, and release

- [ ] Define the acceptance-test contract for each security and lifecycle
      invariant.
- [ ] Define failure-injection tests for sandbox, provider, supervisor,
      persistence, and interrupted writes.
- [ ] Define supported toolchains, operating systems, terminals, and provider
      versions.
- [ ] Define compatibility and migration policy for configuration, resources,
      memory, and sessions.
- [ ] Define packaging, installation, upgrade, uninstall, and rollback.
- [ ] Define performance budgets for startup, discovery, TUI rendering,
      streaming, and concurrent sessions.
- [ ] Define observability requirements without logging secrets or excessive
      sensitive context.

## 14. Decision governance

- [ ] Define what qualifies as an accepted ADR versus an implementation
      specification.
- [ ] Define who can reopen an accepted decision and how supersession is
      recorded.
- [ ] Define the required evidence before implementation starts: examples,
      acceptance criteria, threat cases, and unresolved ambiguity threshold.
- [ ] Define how this register is kept synchronized with ADRs and specs.
- [ ] Define the point at which the design phase ends and implementation is
      explicitly authorized.

## Explicitly deferred by Specification 0001

These are already named as non-blocking follow-ups in the current proposed
specification and remain open until separately decided:

- [ ] Closed provider `kind` catalog.
- [ ] Command-template argument syntax.
- [ ] Resource dependency edges and resolution behavior.
- [ ] Persistent trust, approval, audit, and session file schemas.

## Working rule

When one item is decided, record the decision in the relevant ADR or
specification and link it from this register. Do not mark an item complete
because an implementation exists; the design decision must be explicit first.
