# Risks and accepted costs

Each line is a cost [ADR 0001](../decisions/0001-architecture.md) or [ADR 0002](../decisions/0002-stack.md)
accepted. None is tracked by an issue yet: the first sprint plan opens them.

| Risk | Consequence |
| --- | --- |
| Daemon lifecycle: start, reattach, idle exit, stale socket | It is Milestone 1 work that comes before any feature |
| Every client–engine interaction is serialized | Overhead even on one machine |
| ACP's shape constrains the protocol | Anything outside it becomes a `_kiri/*` extension method |
| The Claude subscription's loop is not Kiri's | The gate must work through the CLI's hooks, and behavior follows the CLI's release cadence |
| No in-process code plugins | A plugin that needs code must ship an MCP server |
| The gate and persistent memory have no module | Their specs must place them before code |
| The Windows sandbox: a dedicated account, WFP filters, and ACLs | The largest piece of Milestone 1, with a one-time elevated setup |
| A fixed self-signed certificate may not keep the macOS Keychain silent across updates | A spike before Distribution; the fallback is a paid Developer ID |
| Smart App Control can block an unsigned `kiri.exe` | A spike before Distribution; Windows code signing is not available to individuals outside the USA and Canada |
| `crossterm` releases slowly; Codex carries four fixes in a fork | The TUI spec checks those cases: color queries, input typed during startup, device-attribute replies, Escape on Windows Terminal |
