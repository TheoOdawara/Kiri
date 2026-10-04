# Persistence

Everything Kiri stores is text: JSONL, TOML, or Markdown. There is no database. What is stored is the
[data model](../../data-model/README.md).

| Location | Holds |
| --- | --- |
| `~/.kiri` | The user's sessions, configuration, memory, and extensions |
| `<project>/.kiri` | The project's configuration and extensions |

Text is the choice because sync goes through git: text diffs and merges where a database file conflicts
whole. Secrets are the exception: they stay out of both locations
([NFR-SEC-01](../../requirements/non-functional/security.md#nfr-sec-01)).
