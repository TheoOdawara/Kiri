# Security

| Mechanism | What it guarantees | Requirement | Milestone |
| --- | --- | --- | --- |
| Approval modes | Plan refuses side effects; default asks; auto acts inside its bounds | [FR-SAF-01 to 03](../../requirements/functional/safety.md) | M1 |
| Permissions | Each action runs, asks, or is blocked by the user's rules | [FR-SAF-04](../../requirements/functional/safety.md#fr-saf-04) | M1 |
| Trust approval | Nothing from a project or plugin activates before the user approves | [FR-SAF-08](../../requirements/functional/safety.md#fr-saf-08), [BR-02](../../requirements/business-rules.md#br-02) | M1 |
| Tool output is data | Output never changes mode, permissions, or configuration | [BR-01](../../requirements/business-rules.md#br-01) | M1 |
| Secrets | Keys and tokens stay out of configuration, logs, and transcripts | [NFR-SEC-01](../../requirements/non-functional/security.md#nfr-sec-01) | M1 |
| Owner-only transport | Another OS user cannot reach the daemon | [NFR-SEC-02](../../requirements/non-functional/security.md#nfr-sec-02) | M1 |
| Process isolation | Third-party code runs only as a child process | [ADR 0001](../../decisions/0001-architecture.md) | Later |
| OS confinement | A command cannot write outside the allowed area | [FR-SAF-07](../../requirements/functional/safety.md#fr-saf-07) | Later |
| Classifier | An optional model judges each action in auto mode | [FR-SAF-05](../../requirements/functional/safety.md#fr-saf-05) | Later |

Until OS confinement ships, permissions are the only bound of auto mode. Where secrets are stored is
[OQ-01](../../requirements/open-questions.md).
