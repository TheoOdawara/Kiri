---
name: Commit
description: Write Conventional Commits and get explicit approval before committing — show `git status` and the diff first, one logical change per commit. Trigger whenever the user asks to commit, stage changes, or prepare a commit message.
tags:
  - git
  - commit
---

Before committing, run `git status` and `git diff --staged`, show the change, and get explicit approval
— never commit silently.

Subject line: `type(scope): summary` — feat, fix, docs, refactor, test, chore — imperative mood, no
trailing period, 72 characters or fewer.

One logical change per commit; stage deliberately, never blanket-add unrelated churn.

The body explains why the change was made, not what changed line by line; reference related issues in a
footer.

Never commit a secret, token, or credential.
