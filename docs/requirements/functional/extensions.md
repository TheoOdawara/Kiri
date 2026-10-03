# Extensions — `EXT`

What a user adds to Kiri: MCP servers, skills, commands, hooks, subagents, and plugins.

<a id="fr-ext-01"></a>
## FR-EXT-01 — MCP servers

> As an engineer, I want MCP servers exposed as tools, so that the agent reaches systems Kiri does not know.

The system shall expose the tools of a configured MCP server to the agent.

| Attribute | Value |
| --- | --- |
| Rationale | MCP is how code-level extension happens. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-01.1** — Given a trusted MCP server, when a session starts, then the server's tools are callable by the model.


<a id="fr-ext-02"></a>
## FR-EXT-02 — Skills

> As an engineer, I want Markdown files as reusable instructions, so that I write an instruction once.

The system shall load a Markdown skill as instructions the agent applies.

| Attribute | Value |
| --- | --- |
| Rationale | Skills are data, never code. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-02.1** — Given a trusted skill, when a session starts, then the agent can apply it.


<a id="fr-ext-03"></a>
## FR-EXT-03 — Commands

> As an engineer, I want Markdown files as commands, so that I trigger a prepared prompt by name.

The system shall load a Markdown command as a command the user invokes by name.

| Attribute | Value |
| --- | --- |
| Rationale | Commands are data, never code. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-03.1** — Given a trusted command, when the user invokes it by name, then its content is sent as the prompt.


<a id="fr-ext-04"></a>
## FR-EXT-04 — Hooks

> As an engineer, I want my scripts triggered on agent events, so that my workflow is enforced by my own code.

The system shall run a user script as a child process when its agent event occurs.

| Attribute | Value |
| --- | --- |
| Rationale | Hooks bound auto mode with the user's own workflow. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-04.1** — Given a trusted hook registered for an event, when the event occurs, then the script runs.


<a id="fr-ext-05"></a>
## FR-EXT-05 — Subagents

> As an engineer, I want the agent to dispatch isolated subagents, so that a scoped task does not consume the main conversation.

The system shall let the agent dispatch a subagent that runs a scoped task in a session of its own.

| Attribute | Value |
| --- | --- |
| Rationale | A subagent is a child session in the daemon. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-05.1** — Given a subagent definition, when the agent dispatches it with a task, then the task runs in a separate session and its result returns to the agent.


<a id="fr-ext-06"></a>
## FR-EXT-06 — Global and project levels

> As an engineer, I want extensions at a user level and at a project level, so that personal and team extensions stay separate.

The system shall load each kind of extension from a global (user) level and from a project level.

| Attribute | Value |
| --- | --- |
| Rationale | Some extensions follow the person, others the repository. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-06.1** — Given one extension at the global level and one at the project level, when a session starts in that project, then both are usable.
- **FR-EXT-06.2** — Given a project-level extension, when a session starts in another project, then it is not loaded.


<a id="fr-ext-07"></a>
## FR-EXT-07 — Bundled skills

> As an engineer, I want a set of skills built in, so that they work with no setup.

The system shall ship `ponytail` (DietrichGebert/ponytail), `humanizer` (blader/humanizer), `i-have-adhd` (ayghri/i-have-adhd), and `grill-me` (mattpocock/skills) built in.

| Attribute | Value |
| --- | --- |
| Rationale | The author's daily skills. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-07.1** — Given a fresh install, when the first session starts, then every bundled skill is available.


<a id="fr-ext-08"></a>
## FR-EXT-08 — Skill updates from upstream

> As an engineer, I want skills kept current with their source, so that I never update a skill by hand.

The system shall fetch a new version of a bundled or third-party skill from its upstream repository, independently of Kiri's own releases.

| Attribute | Value |
| --- | --- |
| Rationale | A skill moves faster than the binary. |
| Source | Author decision, kickoff 2026-10-03 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-08.1** — Given a skill whose upstream publishes a new version, when Kiri fetches it, then the new version is used with no new Kiri release installed.


<a id="fr-ext-09"></a>
## FR-EXT-09 — Plugins from git marketplaces

> As an engineer, I want to install a plugin from a git marketplace, so that one install brings its skills, commands, agents, hooks, and MCP servers.

The system shall install a plugin, bundling skills, commands, agents, hooks, and MCP servers, from a git marketplace.

| Attribute | Value |
| --- | --- |
| Rationale | Extensions are distributed as plugins. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-09.1** — Given a git marketplace listing a plugin, when the user installs and trusts it, then every extension it bundles is usable.


<a id="fr-ext-10"></a>
## FR-EXT-10 — Subagent attach

> As an engineer, I want to attach to a running subagent, so that I watch and steer it.

The system shall let a client attach to a subagent's session, watch it, and send it messages.

| Attribute | Value |
| --- | --- |
| Rationale | A subagent speaks the same protocol as any session. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-EXT-10.1** — Given a running subagent, when the user attaches to it, then its events are shown and a message the user sends reaches it.
