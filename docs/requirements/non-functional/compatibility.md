# Compatibility — `COMP`

ISO/IEC 25010 characteristic: compatibility.

<a id="nfr-comp-01"></a>
## NFR-COMP-01 — Claude Code extension formats

The system shall load skills, commands, agents, hooks, and MCP configuration written in Claude Code's formats.

| Attribute | Value |
| --- | --- |
| Rationale | That ecosystem works from day one. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-COMP-01.1** — A skill, command, agent, hook, or MCP configuration valid for Claude Code loads in Kiri with 0 modifications.
