# Data model

The conceptual model is inferred from the [requirements](../requirements/README.md) and
[ADR 0001](../decisions/0001-architecture.md). Specs add attributes as they close; no file layout has been decided.

![Conceptual model](diagrams/conceptual.drawio.svg)

## Entities

| Entity | One instance is | Requirements | Milestone |
| --- | --- | --- | --- |
| Project | A directory the user works in | [FR-CNT-03](../requirements/functional/continuity.md#fr-cnt-03) | M1 |
| Session | One conversation, owned by the daemon | [FR-CNT-01](../requirements/functional/continuity.md#fr-cnt-01) | M1 |
| Message | One entry of a session's conversation | [FR-AGT-01](../requirements/functional/agent.md#fr-agt-01) | M1 |
| Provider | One configured source of model output | [FR-PRV-09](../requirements/functional/providers.md#fr-prv-09) | M1 |
| Permission rule | One rule making an action run, ask, or be blocked | [FR-SAF-04](../requirements/functional/safety.md#fr-saf-04) | M1 |
| Checkpoint | A point of a session its file edits can return to | [FR-CNT-04](../requirements/functional/continuity.md#fr-cnt-04) | Later |
| Extension | One skill, command, hook, MCP server, subagent definition, or plugin | [FR-EXT](../requirements/functional/extensions.md) | Later |
| Memory note | One fact or preference kept across sessions | [FR-CNT-05](../requirements/functional/continuity.md#fr-cnt-05) | Later |

## Attributes decided so far

All are in memory; none reaches a file. From the spec of [#2](../specs/streamed-answers-on-the-daemon/streamed-answers-on-the-daemon.md), Milestone M1.

| Entity | Attributes |
| --- | --- |
| Session | `id` (UUIDv7), `cwd`, `messages` |
| Message | `role` (`user` or `assistant`), `text`, optional `reasoning` |
| Provider | `base_url`, `api_key`, `model`, `request_params`; one instance per daemon, from the environment |

## Relations

- A project has zero or more sessions; a session belongs to one project.
- A session holds zero or more messages and marks zero or more checkpoints.
- A session spawns zero or more child sessions: a subagent is a child session.
- A session runs on one provider at a time and on several over its life, because the provider is switchable live.
- A permission rule and an extension are global or scoped to one project.
- A memory note has no relation yet: whether memory is global, per project, or both is not decided.

## Not written yet

- **Logical model** — `diagrams/logical.drawio.svg`, in crow's foot notation, once the specs set each
  entity's attributes and identity.
- **Physical model** — none: Kiri has no database. What is stored is text files
  ([persistence](../architecture/concepts/persistence.md)).
- **Access patterns** — `access-patterns.md`, once each spec sets its operations and volumes.
- **Storage** — `storage.md`: each file's path, format, writer, and lifecycle, as each spec decides which file
  holds which entity.
