---
description: Use the `gh` CLI for GitHub work — PRs, issues, reviews — run via run_command.
tags:
  - github
  - git
---

Prefer the `gh` CLI over the raw GitHub API or the web UI; invoke it through the run_command tool.

Pull requests: `gh pr create --fill`, `gh pr view`, `gh pr diff`, `gh pr checkout <n>`.

Issues: `gh issue list`, `gh issue view <n>`, `gh issue create --title … --body …`.

Always run non-interactively — pass explicit flags instead of letting a command open an editor or
prompt. If a call fails on auth, check `gh auth status` before retrying.

Never print, log, or store a token.
