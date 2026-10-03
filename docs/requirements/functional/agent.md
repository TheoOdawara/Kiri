# Agent — `AGT`

The conversation loop between the user, the model, and the tools.

<a id="fr-agt-01"></a>
## FR-AGT-01 — Multi-turn loop

> As an engineer, I want the agent to keep reasoning and calling tools until it has an answer, so that one prompt completes a task that needs several steps.

The system shall run a turn as a loop in which the model reasons, calls tools, receives their results, and continues until it produces a final answer.

| Attribute | Value |
| --- | --- |
| Rationale | A coding task is rarely one model call. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-AGT-01.1** — Given a configured provider, when the user sends a prompt that needs a tool, then the tool's result is returned to the model and the turn ends with an answer.


<a id="fr-agt-02"></a>
## FR-AGT-02 — Live streaming

> As an engineer, I want to see reasoning, text, and tool calls as they arrive, so that every step of the agent is visible while it happens.

The system shall stream reasoning, text, and tool calls to the interface as they arrive from the provider.

| Attribute | Value |
| --- | --- |
| Rationale | Full control starts with seeing every step. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-AGT-02.1** — Given a configured provider, when the model produces output, then reasoning, text, and tool calls appear in arrival order before the turn ends.


<a id="fr-agt-03"></a>
## FR-AGT-03 — Turn interruption

> As an engineer, I want to interrupt a turn at any moment, so that I stop work that is going wrong without losing the conversation.

The system shall let the user interrupt a running turn at any moment while keeping the conversation.

| Attribute | Value |
| --- | --- |
| Rationale | The user keeps the wheel. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-AGT-03.1** — Given a running turn, when the user interrupts it, then the turn stops and every earlier message of the conversation is available to the next prompt.


<a id="fr-agt-04"></a>
## FR-AGT-04 — Project instructions

> As an engineer, I want the project's instruction files read at session start, so that the agent follows the project's rules without me repeating them.

The system shall load the project's instruction files into the model's context when a session starts.

| Attribute | Value |
| --- | --- |
| Rationale | Project rules belong to the repository, not to each prompt. Which file names are read is [OQ-04](../open-questions.md). |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-AGT-04.1** — Given a project holding an instruction file, when a session starts in it, then the file's content is part of the context of the first turn.
