# 0002. Harness configuration and discovery

- Status: accepted
- Date: 2026-08-03

## Context

Kiri must provide the extensibility of Claude Code with the organization and
predictability associated with Codex. Instructions, skills, hooks, commands,
rules, agents, validators, configuration, and memory have different purposes
and must not be collapsed into one document or one implicit mechanism.

The Kiri harness must remain authoritative over its own integrity while allowing
projects to customize their behavior.

## Decision

### Global scope

The only active global Kiri scope is:

```text
~/.kiri/
```

Kiri does not load global configuration, skills, memory, hooks, or other
artifacts from `~/.claude`, `~/.codex`, or `~/.agents`. Those systems are design
references only. Kiri has no migration or import feature for them.

### Repository scope

Kiri accepts these repository-level instruction files as alternatives, in strict
priority order:

```text
KIRI.md
CLAUDE.md
AGENTS.md
```

Only the first existing file is loaded for a given scope. The files are not
merged. `KIRI.md` is the native format; `CLAUDE.md` and `AGENTS.md` are
compatibility fallbacks because they represent the same instruction role.

Kiri accepts these repository directories:

```text
.kiri/
.agents/
```

`.kiri/` is the native Kiri directory. `.agents/` is the interoperable agent
directory. When both provide equivalent artifacts, `.kiri/` has priority.

Kiri does not load `.claude/` or `.codex/` repository directories.

### Layering

Configuration layers resolve in this order:

```text
built-in defaults < ~/.kiri/ < repository configuration
```

Global configuration provides defaults. Repository configuration can add to and
override permitted behavior for that project.

Workflow components follow the same ownership rule:

- A component with a new ID is added.
- A repository component with the same ID replaces the global workflow component.
- `.kiri/` wins over `.agents/` when both define equivalent repository artifacts.
- This override applies to workflow behavior, including instructions, skills,
  commands, rules, agents, and non-mandatory hooks.
- Security policy, sandbox boundaries, credential handling, and mandatory
  validators remain outside repository override authority.

### Harness components

The harness keeps these components separate:

- Instructions describe project context and behavioral guidance.
- Skills describe reusable workflows and may reference supporting resources.
- Commands provide explicit user-invoked entry points.
- Rules provide scoped behavioral constraints.
- Agents define focused roles and boundaries.
- Hooks and validators provide deterministic checks and enforcement.
- Memory stores accumulated project or global learnings and is not project
  documentation.
- Configuration selects permitted behavior without becoming the implementation
  of the harness itself.

Documentation is normative project knowledge. Memory is operational context
accumulated by the harness. Neither replaces the other.

### Configuration integrity

Repository configuration may customize workflow behavior and request filesystem
access within the sandbox:

- It can add or override workflow components by stable ID.
- It can request access to files and directories.
- It can request access outside the repository root.
- External access requires user approval and may be persisted.
- It cannot grant access to itself.
- It cannot change network access, the available tool surface, command policy,
  mandatory hooks, or validators.
- It cannot disable or weaken protected harness invariants, mandatory hooks, or
  mandatory validators.

Persistent grants are stored globally under `~/.kiri/` and are scoped to the
project, canonical path, and access capability.

Filesystem permission prompts expose exactly four outcomes:

- `Permitir`: allow the current operation only.
- `Permitir sempre`: persist an approval for the scoped project, path, and
  capability.
- `Negar`: reject the current operation.
- `Responder...`: send free-form text to the agent without granting
  permission.

Free-form text is never interpreted as implicit authorization.

### Configuration UI

The UI is a validated editor over the individual configuration layers. It
shows the resolved effective configuration, but edits one selected layer at a
time:

- Global edits write `~/.kiri/config.toml`.
- Repository edits write `.kiri/config.toml`.
- Values protected from repository override are displayed as read-only in the
  repository layer.
- Changes are validated before writing and take effect immediately when the
  setting permits live application.
- The UI never writes a generated merged configuration and never discards
  unknown configuration keys.

### Resource selection

Agents, skills, commands, hooks, validators, and other discovered resources
declare their identity, description, category, scope, and default activation
policy in their own YAML frontmatter. The configuration layer stores user
overrides by stable resource ID.

The UI provides a central selector for these resources with search, category
filtering, active/inactive state, origin, and reset-to-default controls. It
edits the selected configuration layer rather than rewriting a resource's
content or frontmatter. Mandatory security resources are visible but locked.

The effective state shows whether it comes from the resource default, global
configuration, or repository configuration. Repository overrides remain
subject to the harness invariants and cannot disable mandatory resources.

An explicit user override in the global configuration is a personal veto. In
particular, a global `disabled` choice cannot be silently reversed by a
repository frontmatter default or repository configuration.

## Consequences

- Kiri has one native global configuration model and one native repository model.
- Existing Claude and Codex directory conventions cannot silently change Kiri's
  behavior.
- Projects can customize workflow and filesystem access while the harness
  retains authority over security-sensitive behavior.
- Compatibility with instruction files is simple and deterministic.
- Memory must have its own storage, lifecycle, and validation rules rather than
  being inferred from documentation changes.
- The exact on-disk schemas for skills, agents, hooks, commands, rules, and
  memory remain implementation work and are not implied by this ADR.

## Alternatives considered

- Loading `~/.claude` and `~/.codex` directly was rejected because global Kiri
  behavior must be owned by Kiri.
- Loading `.claude/` and `.codex/` directly was rejected because their hooks,
  permissions, and configuration semantics are outside Kiri's authority.
- Merging `KIRI.md`, `CLAUDE.md`, and `AGENTS.md` was rejected because silent
  instruction conflicts are difficult to reason about.
- Allowing repository configuration to expand network, tools, or command
  permissions was rejected because it would let a project weaken the harness.
- Treating documentation as memory was rejected because normative project facts
  and agent-generated operational learnings have different ownership and
  lifecycles.
