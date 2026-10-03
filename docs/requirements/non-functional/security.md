# Security — `SEC`

ISO/IEC 25010 characteristic: security.

<a id="nfr-sec-01"></a>
## NFR-SEC-01 — Secrets stay out of files and logs

The system shall keep API keys and tokens out of configuration files, logs, and transcripts.

| Attribute | Value |
| --- | --- |
| Rationale | `~/.kiri` is synced through git; a secret written there leaks. Where secrets are stored is [OQ-01](../open-questions.md). |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-SEC-01.1** — After a session that used an API key, 0 occurrences of the key exist in any configuration file, log, or transcript.


<a id="nfr-sec-02"></a>
## NFR-SEC-02 — Owner-only transport

The system shall restrict the daemon's transport to the operating-system user who owns it.

| Attribute | Value |
| --- | --- |
| Rationale | The daemon executes commands; another local user must not reach it. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-SEC-02.1** — Given the daemon of one OS user, when another OS user tries to connect, then 0 connections succeed.
