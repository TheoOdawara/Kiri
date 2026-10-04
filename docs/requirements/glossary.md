# Glossary

| Term | Meaning |
| --- | --- |
| ACP | Agent Client Protocol: the JSON-RPC protocol every client speaks to the daemon. |
| Agent backend | A provider whose own loop drives the turn while Kiri executes the tools; today only the Claude subscription. |
| Approval mode | Plan, default, or auto: how much the agent does without asking. |
| Bundled skill | A skill shipped inside Kiri. |
| Checkpoint | A point of the conversation the agent's file edits can be rolled back to. |
| Classifier | An optional model that judges each action in auto mode. |
| Client | A front-end connected to the daemon: an interactive interface, `kiri -p`, or `kiri acp`. |
| Command | A Markdown file the user invokes by name. |
| Daemon | The per-user background process that owns every session. |
| Gate | The repository's own checks, run before the agent may declare work finished. |
| Hook | A user script run as a child process on an agent event. |
| Marketplace | A git repository listing plugins. |
| MCP | Model Context Protocol: a server process whose tools the agent can call. |
| Permission | A rule that makes an action run, ask, or be blocked. |
| Plugin | A bundle of skills, commands, agents, hooks, and MCP servers. |
| Provider | A source of model output: an API, a local server, or a subscription. |
| Sandbox | OS-level confinement of the commands the agent runs. |
| Session | One conversation with its history and state, owned by the daemon. |
| Side effect | Any change outside the conversation: a file write, or a command that changes state. |
| Skill | A Markdown file of reusable instructions. |
| Subagent | A child session the agent dispatches for a scoped task. |
| Trust | The user's explicit approval for a project or plugin to take effect. |
| Turn | One prompt and everything the agent does until it answers. |
