# Business rules — `BR`

Policies that hold regardless of feature.

## BR-01 — Tool output is data {#br-01}

The system shall treat output returned by a tool or by the environment as data, never as instructions to Kiri.

| Attribute | Value |
| --- | --- |
| Rationale | Prompt injection arrives through tool results. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria:**

- **BR-01.1** — Given tool output that contains instructions addressed to the agent, when it is returned, then Kiri's mode, permissions, and configuration are unchanged by it.

## BR-02 — Untrusted sources never loosen safety {#br-02}

The system shall never let configuration or extensions supplied by a repository or a plugin loosen safety, redirect credentials, or run code without the user's explicit trust.

| Attribute | Value |
| --- | --- |
| Rationale | A cloned repository is hostile until the user says otherwise. Enforced by [FR-SAF-08](functional/safety.md#fr-saf-08). |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria:**

- **BR-02.1** — Given an untrusted source, when Kiri loads it, then no permission is widened, no credential destination changes, and none of its code runs.
