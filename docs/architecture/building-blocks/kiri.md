# kiri

The binary. It holds no engine logic: each subcommand wires `kiri-core` and `kiri-tui` into one process role.
Milestone: M1.

| Invocation | Role | Milestone |
| --- | --- | --- |
| `kiri` | Opens the interactive interface as a client; starts the daemon when none runs | M1 |
| `kiri daemon` | Hosts every session of the user; exits after an idle period | M1 |
| `kiri -p` | Headless client: one prompt, text or JSON output | Later |
| `kiri acp` | stdio bridge between an IDE and the daemon's socket | Later |

## What it never does

- It never runs the engine inside a client process: every client goes through the daemon.
