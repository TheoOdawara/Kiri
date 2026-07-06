# ADR 0019 — Instructions Layering

**Status:** Accepted  
**Date:** 2026-06-29  
**Relates to:** ADR 0007 (system prompt), ADR 0012 (config layering)

## Context

Users need a way to give Kiri persistent, project-specific or global behavioural guidance — equivalent to
a `CLAUDE.md` / `AGENTS.md` in other agent harnesses. The guidance must be injected into the session
system prompt so it is present on every turn without the user re-stating it.

## Decision

### Discovery order

At boot, `Settings::resolve` searches for an instructions file in each directory using the precedence:

```
KIRI.md → AGENTS.md → CLAUDE.md
```

The first file found in a directory wins for that layer. Two layers are searched:

| Layer | Directory | Typical use |
|---|---|---|
| Global | `~/.kiri/` | Cross-project preferences |
| Project | workspace root | Per-project rules |

Both layers are loaded and merged (`global\n\n project`). A CLI flag `--instructions <file>` overrides
both layers entirely with the given file's content.

### System prompt placement

The merged instructions text is injected at a single `{INSTRUCTIONS}` placeholder that sits **before
`# Security`**. The Security section must always be the final authority; placing instructions before it
ensures the harness's security policy cannot be downgraded by user-supplied text.

Final prompt shape:
```
[Static sections: Identity … Memory & preferences]
# User Instructions
{merged_text}

# Security
[Security section]
[Memory digest, if any]
```

When no instructions file is found the placeholder expands to the empty string — no extra blank line, no
section header.

Unlike `rules`/`skills` (ADR 0021), which pass through the extensions trust gate — TOFU-approved by the
user before ever loading — instruction files are workspace-authored and loaded unconditionally on every
boot, with no approval step. So the rendered block opens with an explicit caveat: this content is user- or
workspace-supplied guidance, not harness policy, and it cannot loosen, override, or take precedence over
the `# Security` section that follows. This keeps the model from treating a project's `KIRI.md` as
equivalent to the harness's own security posture, however the file is phrased (issue #32).

### TUI surface

`/instructions` (alias `/instrucoes`) shows the active instructions and their source paths as a
transcript notice. View-only in v1; no inline editor.

## Consequences

- Adding a `KIRI.md` at the workspace root or at `~/.kiri/` takes effect on the next session start
  (the prompt is rendered once at boot, not per-turn).
- A `CLAUDE.md` at the workspace root is picked up as a fallback so Kiri is usable in repos that
  already have one without renaming.
- The `--instructions` override is useful for scripted or CI invocations that need a specific prompt.
- The project layer is read from the workspace root only — it is never parsed from `.kiri/` to keep
  the semantics clear: `.kiri/` is harness state, the root is the project contract.
- Issue #12 (audit) closes against this ADR's already-shipped model: the view-only TUI surface and
  file-based editing above are the deliberate decision, not a gap. The actual gap was test coverage —
  `settings.rs` had none of the discovery order, layer-merge, or CLI-override behavior locked by a test;
  `system_prompt.rs` had ordering tests but none with adversarial instructions content. Both closed
  2026-07-05.
- Issue #32 (audit) closes with the non-policy caveat above: `render_system_prompt`'s instructions block
  now states plainly that it is not harness policy and cannot override Security, locked by
  `instructions_block_states_it_is_not_harness_policy`. Closed 2026-07-05.
