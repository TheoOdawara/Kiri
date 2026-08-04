# Kiri-V2 Design Documentation

This directory records decisions for the greenfield Kiri-V2 rewrite.

The current implementation under the repository root is a source of requirements,
known failures, and lessons learned. It is not the architecture for Kiri-V2.

## Decisions

- [0001. Foundation stack](decisions/0001-foundation-stack.md)
- [0002. Harness configuration and discovery](decisions/0002-harness-configuration-and-discovery.md)
- [0003. Execution modes and policy enforcement](decisions/0003-execution-modes-and-policy-enforcement.md)
- [0004. Memory architecture and backends](decisions/0004-memory-architecture-and-backends.md)
- [0005. Live session control](decisions/0005-live-session-control.md)

Only decisions confirmed during the design process belong in the accepted ADRs.
Unresolved design areas remain undocumented until their blocking ambiguities are
resolved. Memory extraction heuristics and the exact on-disk schemas remain
implementation-level design work.
