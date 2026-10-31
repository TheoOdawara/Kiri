# Roadmap

Live status: the [GitHub Project](https://github.com/users/TheoOdawara/projects/7) and the
[milestones](https://github.com/TheoOdawara/Kiri/milestones).

## M1 — Daily core

The core, usable daily: a terminal agent on API-key and local providers, with its tools under approval
modes and OS confinement. GitHub Milestone: [M1](https://github.com/TheoOdawara/Kiri/milestone/1).

Epics are in dependency order.

| Epic | Issue | Covers |
| --- | --- | --- |
| Daemon and reattach | [#2](https://github.com/TheoOdawara/Kiri/issues/2) | FR-CNT-01, FR-CNT-02, FR-CNT-09, NFR-PERF-02, NFR-SEC-02 |
| Branded inline interface | [#3](https://github.com/TheoOdawara/Kiri/issues/3) | FR-UI-01, FR-UI-06, FR-UI-07, NFR-PERF-01, NFR-INTR-01, NFR-INTR-02 |
| Streamed answers | [#4](https://github.com/TheoOdawara/Kiri/issues/4) | FR-AGT-02, FR-PRV-01 |
| Provider choice | [#5](https://github.com/TheoOdawara/Kiri/issues/5) | FR-PRV-02, FR-PRV-03, FR-PRV-04, FR-PRV-08, FR-PRV-09, NFR-SEC-01 |
| Tools and the agent loop | [#6](https://github.com/TheoOdawara/Kiri/issues/6) | FR-AGT-01, FR-AGT-03, FR-TOL-01 to FR-TOL-07, BR-01 |
| OS confinement | [#7](https://github.com/TheoOdawara/Kiri/issues/7) | FR-SAF-07 |
| Approval modes and permissions | [#8](https://github.com/TheoOdawara/Kiri/issues/8) | FR-SAF-01, FR-SAF-02, FR-SAF-03, FR-SAF-04 |
| Project trust | [#9](https://github.com/TheoOdawara/Kiri/issues/9) | FR-SAF-08, FR-AGT-04, BR-02 |
| Long sessions | [#10](https://github.com/TheoOdawara/Kiri/issues/10) | FR-AGT-05, FR-AGT-06 |
| Prompt editing | [#11](https://github.com/TheoOdawara/Kiri/issues/11) | FR-UI-08, FR-UI-09 |
| Platform and failure conformance | [#12](https://github.com/TheoOdawara/Kiri/issues/12) | NFR-FLEX-01, NFR-REL-01, NFR-REL-02, NFR-REL-03 |

Decided while splitting the milestone:

- NFR-FLEX-01 and NFR-REL-01 to 03 apply to every epic. Each spec carries them as constraints, and #12
  closes last, once it has verified them across the whole system.
- CI on Windows, Linux, and macOS arrives with the first code task, so platform problems show up from the
  first commit.
- #6, #7, and #8 enter the same sprint, because edits, writes, and shell commands must not exist between
  sprints without approvals and confinement.

## Requirement coverage

Every approved requirement of a planned milestone maps to exactly one epic.

| Requirement | Epic | Milestone |
| --- | --- | --- |
| FR-AGT-01 | #6 | M1 |
| FR-AGT-02 | #4 | M1 |
| FR-AGT-03 | #6 | M1 |
| FR-AGT-04 | #9 | M1 |
| FR-AGT-05 | #10 | M1 |
| FR-AGT-06 | #10 | M1 |
| FR-PRV-01 | #4 | M1 |
| FR-PRV-02 | #5 | M1 |
| FR-PRV-03 | #5 | M1 |
| FR-PRV-04 | #5 | M1 |
| FR-PRV-08 | #5 | M1 |
| FR-PRV-09 | #5 | M1 |
| FR-TOL-01 | #6 | M1 |
| FR-TOL-02 | #6 | M1 |
| FR-TOL-03 | #6 | M1 |
| FR-TOL-04 | #6 | M1 |
| FR-TOL-05 | #6 | M1 |
| FR-TOL-06 | #6 | M1 |
| FR-TOL-07 | #6 | M1 |
| FR-SAF-01 | #8 | M1 |
| FR-SAF-02 | #8 | M1 |
| FR-SAF-03 | #8 | M1 |
| FR-SAF-04 | #8 | M1 |
| FR-SAF-07 | #7 | M1 |
| FR-SAF-08 | #9 | M1 |
| FR-UI-01 | #3 | M1 |
| FR-UI-06 | #3 | M1 |
| FR-UI-07 | #3 | M1 |
| FR-UI-08 | #11 | M1 |
| FR-UI-09 | #11 | M1 |
| FR-CNT-01 | #2 | M1 |
| FR-CNT-02 | #2 | M1 |
| FR-CNT-09 | #2 | M1 |
| NFR-FLEX-01 | #12 | M1 |
| NFR-REL-01 | #12 | M1 |
| NFR-REL-02 | #12 | M1 |
| NFR-REL-03 | #12 | M1 |
| NFR-PERF-01 | #3 | M1 |
| NFR-PERF-02 | #2 | M1 |
| NFR-SEC-01 | #5 | M1 |
| NFR-SEC-02 | #2 | M1 |
| NFR-INTR-01 | #3 | M1 |
| NFR-INTR-02 | #3 | M1 |
| BR-01 | #6 | M1 |
| BR-02 | #9 | M1 |

## Sprints

- [Sprint 1](sprints/sprint-01.md) — from opening `kiri` to a streamed answer that survives closing the terminal
