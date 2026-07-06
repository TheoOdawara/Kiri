# ADR 0028 — Bundled default extensions: a third, binary-shipped layer

- Status: Accepted
- Date: 2026-07-06
- Amends: ADR 0021 (`0021-extensions-framework-and-trust-gating.md`) — adds a third discovery layer
  beneath the two it defines, rather than restating the resource types or the trust gate it already covers.

## Context

ADR 0021 discovers every extension resource "purely from the filesystem" — user-authored Markdown under
`~/.kiri/` (global, trusted) and `<workspace>/.kiri/` (project, untrusted). That is correct for
user-authored customization, but it means a fresh Kiri install starts with nothing: no skills, no agent
profiles, no rules. The harness is only as useful as the setup a new user has not yet done, unlike
ecosystems (Claude Code's own built-in agents/skills) that ship useful defaults in the binary itself.

## Decision

Add `Layer::Bundled` (`extensions::domain::scope::Layer`) alongside `Global`/`Project`: content compiled
into the binary via `include_str!`, trusted the same way `Global` is (`gate::resolve` auto-approves it;
it is a passive resource type today — skills and agent profiles — so nothing in this set actually reaches
the active-capability gate, but the match stays exhaustive for when a bundled hook/MCP server exists).

Content lives under `extensions/infrastructure/bundled/{skills,agents}/*.md`, registered in a `const
BUNDLED` table in `extensions/infrastructure/bundled.rs` and parsed by `parse_bundled` — the same
`Frontmatter::parse` and id-resolution rule `file_loader::load_one` applies to a disk file, so a bundled
`Resource` is indistinguishable in shape from one loaded off disk. Every downstream consumer
(`skills_index`, `command_bodies`, the `/skills`/`/agents` displays) treats it uniformly with no special
casing.

**Precedence: global > project > bundled.** `file_loader::load_type` folds bundled resources in as a
third pass, `or_insert` into the same per-type id map used for global/project — so a user file of the
same id always overrides a default, at either layer. This preserves the existing global-over-project rule
untouched; bundled is strictly the floor.

**Why injection over seeding.** The alternative was writing default files to `~/.kiri/` on first run
(mirroring `write_starter_config`'s NVIDIA-provider seed). Rejected: a seeded file goes stale the moment a
later Kiri release improves the default, with no version-stamp reconciliation; `include_str!` injection
has no such drift, no first-run I/O, and editability is already served — a user who wants to customize
`plano` drops `~/.kiri/skills/plano.md` and it wins by id.

**Content shipped in this ADR:** four skills (`plano` — planning discipline; `gh` — GitHub CLI usage;
`commit` — Conventional Commits discipline; `ponytail` — the lazy-senior-dev/YAGNI ladder, ported
natively from the ponytail Claude Code plugin) and two read-only agent profiles (`search`, `planning`),
the latter made dispatchable by ADR 0029.

## Consequences

- A fresh Kiri install is never extension-empty: `catalog.skills`/`catalog.agents` always contain at
  least the bundled set, even with no `~/.kiri/` content at all.
- `ExtensionCatalog`, `ExtensionsLoader`, ADR 0021's trust gate, and the `/skills`/`/agents`/`/rules`
  displays are unchanged in shape — `Layer::Bundled` is a new enum variant, not a new code path through
  those consumers.
- Every `match` on `Layer` in the codebase had to become exhaustive over three variants
  (`domain::gate::resolve`, the MCP-approval match in `app::build_mcp_tools`, the hook-approval match in
  `tui::infrastructure::runtime::hook_dispatch`) — all three now route `Bundled` the same as `Global`.
- Adding a new bundled default is one Markdown file plus one line in `bundled.rs`'s `BUNDLED` table — no
  loader change.
- Locked by `bundled.rs`'s guard tests (every entry parses, resolves to its stem, skills carry a
  non-empty description, agents list only read-only tools) and `file_loader.rs`'s precedence tests
  (`empty_dirs_yield_only_the_bundled_defaults`, `user_global_skill_overrides_bundled_default_of_same_id`,
  `project_skill_overrides_bundled_default_of_same_id`).
