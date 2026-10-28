# A turn on the Claude subscription

The one provider whose loop Kiri does not own. Milestone: Later.
Requirements: [FR-PRV-06, 07](../../requirements/functional/providers.md#fr-prv-06).

1. The engineer enables the subscription and confirms the account-risk warning.
2. **provider** checks the installed `claude` CLI against the minimum version.
3. On a prompt, **agent** starts the `claude` CLI as a child process in `-p` stream-json mode, with its
   built-in tools disabled.
4. The daemon serves Kiri's **tools** to the CLI through an MCP server.
5. Claude's loop drives: each tool call arrives through MCP and runs under **permissions** and **sandbox**
   like any other.
6. **agent** translates the CLI's stream into the same ACP events, so a client cannot tell the difference.

The built-in gate has to work through the CLI's hooks on this path.
