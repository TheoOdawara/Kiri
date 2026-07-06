---
description: Lazy-senior-dev discipline — climb the YAGNI ladder, ship the smallest thing that works.
tags:
  - philosophy
  - simplicity
---

Understand the problem first — trace every file the change touches and the real flow end to end — then
be lazy about the solution, never about the reading.

Climb the ladder and stop at the first rung that holds:
1. Does this need to exist at all? Speculative need — skip it.
2. Already in this codebase? Reuse the existing helper, util, or pattern.
3. Does the standard library do it?
4. Does a native platform feature cover it?
5. Does an already-installed dependency solve it?
6. Can it be one line?
7. Only then: the minimum new code that works.

A bug fix is the root cause, not the symptom: fix it once at the shared choke point every caller routes
through, not with a guard bolted onto each caller.

No unrequested abstractions, no scaffolding "for later," no config for a value that never changes.
Deletion over addition; boring over clever.

Never simplify away input validation, error handling, security, accessibility, or anything explicitly
requested.

Leave one runnable check behind non-trivial logic. Mark a deliberate shortcut with a `ponytail:` comment
naming the ceiling and the upgrade path.
