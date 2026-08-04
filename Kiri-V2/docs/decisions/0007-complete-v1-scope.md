# 0007. Complete Kiri-V2 v1 scope

- Status: accepted
- Date: 2026-08-04

## Context

Kiri-V2 is a ground-up redesign of Kiri. The purpose of the version is to
produce a better harness under deliberate user supervision, with architecture,
security, and product behavior decided before implementation.

Calling the first usable release a `v1` must not turn the project into a
reduced MVP that omits agreed Kiri capabilities merely to ship sooner.

## Decision

The scope of Kiri-V2 v1 is the complete Kiri product defined by the decisions
made in `Kiri-V2`. v1 is complete only when the agreed Kiri capabilities have
been designed, implemented, integrated, and verified.

This does not require implementing unspecified or hypothetical features. A
capability enters the v1 scope only through an explicit Kiri-V2 decision. The
non-goal is therefore a reduced MVP scope, not a particular Kiri capability
that has already been agreed.

Implementation may be staged internally, but staging does not redefine v1 as
a partial product or authorize skipping design decisions and verification.

## Consequences

- The design register covers the whole Kiri-V2 product, not only a launch slice.
- Every agreed capability must receive a contract and verification path before
  Kiri-V2 v1 is considered complete.
- The project may take longer than an MVP, but it avoids inheriting ad hoc
  behavior or using implementation speed to bypass product and architecture
  decisions.
- Future or hypothetical features remain outside scope until explicitly
  decided.

## Alternatives considered

- A reduced MVP that excludes agreed capabilities was rejected because v1 is the
  complete Kiri-V2 product, not a trial subset.
- Incrementally reshaping the legacy implementation was rejected because the
  purpose of Kiri-V2 is a controlled redesign from the root.
