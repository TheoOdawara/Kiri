# Persistence

Everything Kiri stores is text: JSONL, TOML, or Markdown. There is no database. What is stored is the
[data model](../../data-model/README.md).

| Location | Holds |
| --- | --- |
| `~/.kiri` | The user's sessions, configuration, memory, and extensions; the daemon's `daemon.lock`, `daemon.sock` on Linux and macOS, and `logs/daemon.jsonl` |
| `<project>/.kiri` | The project's configuration and extensions |

Sessions stay in memory until [FR-CNT-03](../../requirements/functional/continuity.md#fr-cnt-03) ships.

Text is the choice because the user reads and edits these files with any editor, and a session's JSONL only
appends. Secrets stay out of configuration, logs, and transcripts
([NFR-SEC-01](../../requirements/non-functional/security.md#nfr-sec-01)): they live in the macOS Keychain, or in
`~/.kiri/credentials.json`, readable only by its owner and encrypted with DPAPI on Windows
([ADR 0002](../../decisions/0002-stack.md)).
