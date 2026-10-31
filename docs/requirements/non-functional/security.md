# Security — `SEC`

ISO/IEC 25010 characteristic: security.

<a id="nfr-sec-01"></a>
## NFR-SEC-01 — Secrets stay out of files and logs

The system shall keep the API keys and tokens it holds out of configuration files, logs, and transcripts.

| Attribute | Value |
| --- | --- |
| Rationale | Configuration files, logs, and transcripts get shared, pasted into issues, and copied between machines; a secret in them leaks. Where secrets are stored is decided in [ADR 0002](../../decisions/0002-stack.md). |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **NFR-SEC-01.1** — After a session that used an API key Kiri holds, 0 occurrences of that key exist in any configuration file, log, or transcript, including where a tool's output contained it.


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
