# 0006. Primary user journey

- Status: accepted
- Date: 2026-08-04

## Context

Kiri must provide value immediately from the directory where the user wants to
work. The user must be able to supervise an active model session, steer it
while it runs, and understand what it is doing without making every progress
detail a blocking interaction.

## Decision

The primary Kiri journey is:

1. The user runs `kiri` from the directory they want to work in.
2. Kiri asks whether the user trusts that directory. A positive answer is
   remembered for that directory, so the user is not asked again for the same
   trusted directory. Trust identity, storage, expiry, revocation, and reset
   rules remain part of the policy decisions.
3. The user can choose or change the execution mode without a blocking setup
   step. The user is free to send a request immediately and can change the
   mode while the session is active.
4. The interface continuously shows user-facing progress and activity, such as
   reads and runs, while the model works. A `thinking` display can be expanded
   or collapsed by activating its indicator or pressing `Ctrl+O`. Repeating the
   action collapses it again. The exact content and presentation rules for
   model thinking remain a separate decision.
5. Mode-specific interaction rules determine which questions appear during the
   session. In `Plan`, the model asks the user for decisions while planning. In
   `Auto`, an action that diverges from the project context asks the user before
   it is performed. These decision questions are distinct from acceptance or
   permission prompts. The exact rules for what triggers each question remain
   open.
6. The user can send messages while the model is executing. Messages enter the
   session input queue and are considered at the first safe point during
   execution; they do not wait for the entire execution to finish. Skills and
   agents can be invoked both in the initial request and during an active
   session. The priority and fairness rules among multiple queued messages
   remain open in the live-session decisions.
7. When the work finishes, Kiri presents a completion report to the user.

The session remains live and controllable throughout this journey. Session
supervision, queueing, pause, stop, and reconnection follow ADR 0005. Policy
evaluation and execution authorization follow ADR 0003.

## Consequences

- Kiri's first usable journey starts in the user's target directory and does
  not require a separate setup wizard.
- Mode selection is a live session control, not a one-time startup gate.
- Progress, activity, optional expanded thinking, user questions, queued input,
  and the final report are all part of the product experience.
- The exact mode-question matrix, trust lifecycle, and thinking presentation
  still need their own decisions.

## Alternatives considered

- Requiring the user to select a mode before sending the first request was
  rejected because mode selection must not block the first interaction.
- Showing only the final result was rejected because the user needs to inspect
  and steer ongoing work.
- Disallowing input while execution is active was rejected because live
  sessions must remain steerable.
