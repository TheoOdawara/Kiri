# Kiri-V2 on-disk schemas

- Status: proposed
- Date: 2026-08-04

## Problem and goal

Kiri needs stable, inspectable formats for user configuration, workflow
resources, and durable memory before implementation starts. The formats must
preserve the boundaries accepted by ADRs 0002–0005:

- configuration is layered but never persisted as a generated merge;
- repository content can customize workflow behavior but cannot weaken the
  harness security boundary;
- resource identity is stable and independent of filenames;
- memory is separate from documentation and carries lineage metadata;
- secrets, approvals, trust, audit records, and session state are not stored
  in workflow files.

This specification fixes the v1 wire shape. Runtime behavior that consumes a
field remains governed by the accepted ADRs and the policy engine.

## Non-goals

- Defining provider-specific request formats or credential backend adapters.
- Defining the supervisor IPC protocol or the SQLite session schema.
- Defining a user-authored validator catalog or MCP server schema.
- Importing Claude, Codex, or agent configuration.
- Making repository files an authority over sandbox, network, credential, or
  mandatory-validator policy.

## Proposed design

### 1. File boundaries

Kiri reads these locations:

```text
~/.kiri/config.toml                         global configuration
~/.kiri/credentials.json                    file credential backend
~/.kiri/{skills,commands,rules,agents,hooks}/*.md

<project>/KIRI.md                            native instructions
<project>/CLAUDE.md                          compatibility fallback
<project>/AGENTS.md                          compatibility fallback
<project>/.kiri/config.toml                  project configuration
<project>/.kiri/{skills,commands,rules,agents,hooks}/*.md
<project>/.agents/{skills,commands,rules,agents,hooks}/*.md
<project>/.kiri/memory/shared/*.md           versionable project memory
```

Only the first existing instruction file in the `KIRI.md` → `CLAUDE.md` →
`AGENTS.md` order is loaded. Instruction files are plain Markdown and have no
frontmatter contract.

`.kiri/` resources take precedence over equivalent `.agents/` resources. A
project resource replaces a global resource with the same stable ID and kind.
Bundled resources use the same frontmatter schema but are compiled into Kiri,
not loaded from these directories.

Project-private memory and the active global memory backend are stored below
Kiri-owned paths. Their exact directory names are runtime details; their entry
format is fixed in section 4.

### 2. Configuration TOML

Each `config.toml` is one layer. The effective configuration is computed as:

```text
built-in defaults < ~/.kiri/config.toml < <project>/.kiri/config.toml
```

The project layer may only contribute fields marked `project: yes` in the
table below. A project value that attempts to change a global-only field is
reported and ignored.

| Key | Type | Required | Project | Meaning |
| --- | --- | --- | --- | --- |
| `version` | positive integer | yes | yes | Schema version; v1 is `1`. |
| `active_provider` | non-empty string | no | no | Provider profile ID for new sessions. |
| `default_mode` | `plan`, `default`, `auto` | no | no | Mode used when a new session starts. |
| `[session].exit_policy` | `detach`, `pause`, `stop` | no | no | Action for active sessions when the TUI closes. |
| `[providers.<id>]` | provider table | no | no | Non-secret provider connection profile. |
| `[credentials].backend` | `file` or `keyring` | no | no | General credential backend; defaults to `file`. |
| `[policy].network` | `no_network`, `read_remote`, `write_remote` | no | no | Global network capability ceiling. |
| `[limits]` | limit table | no | lower only | User/project concurrency limits, never above Kiri's hard ceiling. |
| `[memory]` | memory table | no | no | Active global backend and memory policies. |
| `[resources."<id>"].enabled` | boolean | no | yes | User override for one discovered resource. |
| `[[filesystem.requests]]` | request table | no | yes | Requested project filesystem access, always subject to approval. |

Provider profiles use this shape:

```toml
version = 1
active_provider = "local"
default_mode = "default"

[session]
exit_policy = "pause"

[providers.local]
kind = "openai-compatible"
base_url = "http://localhost:11434/v1"
model = "local-model"

[credentials]
backend = "file"

[policy]
network = "no_network"

[limits]
max_active_sessions = 4
max_project_sessions = 2
max_child_agents = 4

[memory]
backend = "kiri_only"

[memory.resources.global]
policy = "explicit"

[memory.resources.project_shared]
policy = "explicit"

[memory.resources.project_private]
policy = "explicit"

[resources."kiri.planning"]
enabled = true
```

Project-only settings use a separate layer file:

```toml
version = 1

[[filesystem.requests]]
path = "../shared-docs"
capabilities = ["read"]
```

#### Provider table

`[providers.<id>]` requires `kind`, `base_url`, and `model`. `credential` is
an opaque credential reference and is optional only for providers whose adapter
does not require credentials. Raw tokens, API keys, and environment values are
never valid fields in this table.

Provider IDs are unique within the configuration layer and match
`[a-z0-9][a-z0-9._-]{0,63}`. The provider adapter validates `kind`, the URL,
and credential requirements before a session starts.

#### Credential storage

`credential` is an opaque reference to the general `CredentialStore`; it is
not the credential value. Profiles for keyless local providers omit the field.
The global `[credentials].backend` selects `file` or `keyring`, and defaults to
`file`. The project layer cannot override it.

The file backend stores values in `~/.kiri/credentials.json`. The keyring
backend stores the same references in the native operating system credential
manager. Backend changes migrate all entries, verify the destination, and
activate the new backend only after verification. An unavailable selected
backend is an explicit error; Kiri never silently falls back to the other
backend.

#### Built-in local providers

The v1 built-in local provider profiles are Ollama and LM Studio. Both use the
shared OpenAI-compatible adapter. Their default base URLs are
`http://localhost:11434/v1` and `http://localhost:1234/v1`, respectively.

These profiles omit `credential` by default and are available during provider
selection without API-key onboarding. A user may add an opaque credential
reference when the local server requires authentication. The provider catalog
remains responsible for validating the configured URL, model, and capabilities.

#### Limits

The v1 limit keys are:

- `max_active_sessions`: global active-session limit;
- `max_project_sessions`: active-session limit per project;
- `max_child_agents`: child-agent limit per session.

Values are positive integers. A project may lower these values but may not
raise them. Kiri's compiled hard ceilings always win.

#### Memory policy

`[memory].backend` is `kiri_only` or `cross_harness`, with `kiri_only` as the
default. Each memory resource policy is `disabled`, `silent`, or `explicit`.
The resource names are fixed: `global`, `project_shared`, and
`project_private`.

Changing `backend` is a migration and must verify the destination before
changing the active backend. It is never a pointer-only update.

#### Resource overrides

Resource overrides contain only `enabled = true|false`. Absence means “use the
resource default.” An override for an unknown ID is retained and reported as
an orphan; it does not create a resource. A global `enabled = false` is a
personal veto and cannot be reversed by a project layer or resource
frontmatter. Mandatory resources cannot be disabled.

#### Filesystem requests

Each request has exactly:

```toml
[[filesystem.requests]]
path = "../shared-docs"
capabilities = ["read", "write"]
```

`path` is resolved relative to the project and `capabilities` contains one or
more of `read`, `write`, or `delete`, without duplicates. The table is a
request, not a grant. Kiri asks for approval and scopes any persistent grant
to the project, canonical path, and capabilities.

#### Configuration validation and preservation

- Global TOML with invalid syntax or invalid field types fails boot with the
  file path and field name.
- Invalid project TOML is ignored with a visible warning; a repository cannot
  turn a malformed file into an availability denial.
- Unknown keys are preserved by the configuration editor and surfaced as
  warnings. They have no effect until a later schema recognizes them.
- Writes are atomic and never include credentials.
- A selected credential backend that is unavailable is a visible error; Kiri
  does not silently select the other backend.
- The UI edits one source layer at a time and never writes a generated merged
  file.

### 3. Workflow-resource frontmatter

Every resource is a Markdown file with one YAML frontmatter document between
the first `---` line and the closing `---` line. The body after the closing
fence is the resource content.

The common frontmatter fields are:

| Key | Type | Required | Constraint |
| --- | --- | --- | --- |
| `id` | string | yes | Stable across renames; `[a-z0-9][a-z0-9._-]{0,127}`. |
| `kind` | enum | yes | `skill`, `command`, `rule`, `agent`, or `hook`. |
| `name` | string | yes | Human-readable display name. |
| `description` | string | yes | Short selector/index description. |
| `category` | string | yes | Non-empty catalog category. |
| `scope` | enum | yes | `global`, `project`, or `bundled`; must match the discovery source. |
| `activation` | enum | yes | `enabled`, `disabled`, or `mandatory`. Only bundled resources may be mandatory. |
| `tags` | list of strings | no | Unique, non-empty tags. |
| `license` | string | no | Provenance metadata only. |
| `source` | URL string | no | Provenance metadata only. |
| `credit` | string | no | Provenance metadata only. |

Resource IDs are unique across all kinds in the effective catalog. The
filename is not identity. A kind mismatch between the directory and
frontmatter is an error. Unknown keys are validation errors, so a typo cannot
silently change behavior.

Example:

```markdown
---
id: kiri.planning
kind: agent
name: Planning Specialist
description: Explore the repository and produce a read-only implementation plan.
category: workflow
scope: bundled
activation: mandatory
allowed-tools:
  - read_file
  - list_dir
  - search
---

You are a read-only planning specialist.
```

Type-specific fields are:

| Kind | Additional fields | Body |
| --- | --- | --- |
| `skill` | none in v1 | Reusable instructions loaded on demand. |
| `command` | `command` (slash-prefixed), optional `aliases`, optional `agent`, optional `model`, optional `allowed-tools` | Prompt template invoked by the user. |
| `rule` | optional `always` boolean, default `false` | Behavioral rule; always-on rules enter the system context. |
| `agent` | optional `model`, optional `allowed-tools` | Agent system instructions. |
| `hook` | required `event`, optional `matcher` | Shell command text, executed only after trust and policy checks. |

Hook `event` values are `SessionStart`, `SessionEnd`, `TurnEnd`, `PreToolUse`,
and `PostToolUse`. A project hook is never trusted merely because it was
discovered. `matcher` narrows the event; it does not bypass policy
classification.

`allowed-tools` is an upper bound for the resource's request. It is not a
permission grant and cannot bypass the `PolicyEngine`.

Skills have no `script` field in v1. A workflow that needs execution uses an
explicit command or hook, making the active capability visible to trust and
policy evaluation.

The effective activation order is:

```text
mandatory resource
  > global explicit override
  > project explicit override
  > resource frontmatter activation
```

Project resources still require project trust when they can execute or invoke
external capabilities. Discovery never executes a body.

### 4. Memory Markdown frontmatter

Durable memory entries use YAML frontmatter with the same `---` delimiters.
They are not workflow resources and are never injected as authoritative
instructions.

```markdown
---
id: 0190c8c0-6f4f-7c1a-8c12-4b5ef8d83f11
type: decision
project: kiri
visibility: shared
owner: kiri
origin: kiri
copied_from: null
date: 2026-08-04
updated_at: 2026-08-04T15:04:05Z
tags:
  - architecture
  - schemas
---

Memory content is contextual evidence. Verify it against the repository.
```

| Key | Type | Required | Constraint |
| --- | --- | --- | --- |
| `id` | UUIDv7 string | yes | Stable for the entry's complete lineage. |
| `type` | enum | yes | `decision`, `pattern`, `anti-pattern`, `snippet`, `heuristic`, `fact`, or `preference`. |
| `project` | string or `null` | yes | Project ID for project memory; `null` for global memory. |
| `visibility` | enum | yes | `global`, `shared`, or `private`. |
| `owner` | identifier string | yes | `[a-z0-9][a-z0-9._-]{0,63}`. |
| `origin` | identifier string | yes | Producer or migration source using the same identifier form. |
| `copied_from` | UUIDv7 string or `null` | yes | Source entry ID for a snapshot copy. |
| `date` | ISO date | yes | Creation date in UTC. |
| `updated_at` | RFC 3339 timestamp | yes | Last content or metadata update in UTC. |
| `tags` | list of strings | no | Unique search tags. |

The body must be non-empty. `visibility = global` requires `project = null`;
`shared` and `private` require a project ID. `copied_from` is `null` for an
original entry and points to the source ID for a snapshot copy. Copies are
independent; they do not synchronize automatically.

The memory write gate remains mandatory: a fact must be durable, not derivable
from the repository, confirmed or explicitly requested, and not a duplicate.
Conflicts require user choice; deletion is always explicit.

## API and schema changes

This is a storage-only contract. The implementation will add typed Rust
representations for:

- one-layer configuration and layer-aware validation;
- common and kind-specific workflow frontmatter;
- memory frontmatter and lineage metadata;
- credential backend selection and opaque credential references.

The parser must return field-level diagnostics with path, key, and reason. It
must not execute resource bodies while parsing or discovering them.

## Acceptance criteria

- Given a valid global `config.toml`, when Kiri loads it, then provider profiles,
  mode, limits, memory policy, and resource overrides are available as typed
  values without secrets.
- Given a project config that tries to enable network or replace a provider,
  when Kiri resolves layers, then the value is rejected or ignored and the
  global security configuration remains unchanged.
- Given a project resource with the same `(kind, id)` as a global resource,
  when discovery completes, then the project resource is selected while a
  global explicit disable remains effective.
- Given a resource whose `kind`, `scope`, or required field is invalid, when
  discovery runs, then Kiri reports the file and field and never executes its
  body.
- Given a valid memory entry, when Kiri loads it, then its visibility and
  project invariants are enforced and its body remains contextual data.
- Given a backend migration with a shared memory entry, when the destination
  is verified, then Kiri writes an independent snapshot with `copied_from`
  lineage before changing the active backend.
- Given a configuration edit containing unknown keys, when the UI writes the
  selected layer, then those keys remain present and are still surfaced as
  warnings.
- Given a provider profile with an opaque credential reference, when Kiri loads
  it, then the selected global credential backend resolves the value without
  exposing it in configuration or session data.
- Given a backend switch, when every credential is copied and verified, then
  Kiri activates the destination backend and preserves the credential
  references used by provider profiles.
- Given an unavailable selected credential backend, when Kiri resolves a
  credential, then it reports an actionable error and does not silently use the
  other backend.
- Given a fresh installation with a reachable Ollama or LM Studio server, when
  the user selects the local provider, then Kiri does not require an API key or
  create a credential entry before model discovery.

## Deferred, non-blocking follow-ups

- The provider catalog will define the closed set of provider `kind` values.
- The command template argument syntax will be defined with the command
  catalog.
- Resource dependency edges (`requires`) will be defined with the skill/agent/
  hook dependency design; v1 does not infer dependencies from prose.
- Persistent trust, approval, audit, and session files will get schemas in
  their respective implementation specifications.

These follow-ups do not change the fields or invariants defined here.
