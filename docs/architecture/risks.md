# Risks and accepted costs

Each line is a cost [ADR 0001](../decisions/0001-architecture.md) accepted. None is tracked by an issue yet.

| Risk | Consequence |
| --- | --- |
| Daemon lifecycle: start, reattach, idle exit, stale socket | It is Milestone 1 work that comes before any feature |
| Every client–engine interaction is serialized | Overhead even on one machine |
| ACP's shape constrains the protocol | Anything outside it becomes a `kiri/*` extension method |
| The Claude subscription's loop is not Kiri's | The gate must work through the CLI's hooks, and behavior follows the CLI's release cadence |
| No in-process code plugins | A plugin that needs code must ship an MCP server |
| Auto mode in Milestone 1 has no OS confinement | Permissions alone bound it until the sandbox ships |
| The gate and persistent memory have no module | Their specs must place them before code |
