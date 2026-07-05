# ADR 0021 — Extensions Framework and Trust Gating

**Status:** Accepted
**Date:** 2026-06-29
**Relates to:** ADR 0007 (system prompt), ADR 0012 (config layering), ADR 0019 (instructions layering)

## Context

Kiri supports a full AI-workflow surface: **rules**, **commands**, **agents**, **skills**, **hooks**, and
**MCP** servers, each with a **global** (`~/.kiri/`) and a **local/project** (`<workspace>/.kiri/`) layer,
mirroring the two-layer model already used for instructions (ADR 0019) and config (ADR 0012).

The central tension: the project layer lives inside an **untrusted** workspace (a repo a user `cd`s into),
and ADR 0012 locks it to contributing only `effort` — it must not redirect a credential, weaken the sandbox,
or inject env. Yet the *value* of a workflow framework is precisely that teams commit these files to a repo
to share them. So most extension types need to be loadable from the project layer.

## Decision

### Extension types, partitioned by capability

Each extension is classified by the kind of effect it has, and the **project layer's rights** depend on it:

| Class | Examples | Project layer |
|---|---|---|
| **Passive content** (text injected into a prompt or expanded into a prompt) | rules, command prompts, skill instructions, agent system-prompts | Loaded by default and merged into the session system prompt **before `# Security`** — the same posture as `KIRI.md` (ADR 0019). Injecting text does not execute anything; the Security section always takes precedence. |
| **Active capability** (executes a shell/process, opens a network connection, or restricts/extends the toolset) | hooks (shell), MCP (spawn process / open a connection), sub-agents (tool subset + isolation), skill scripts | **Discovered** from the project, but **gated**: activated only after explicit user approval at boot, surfaced as a `BootNotice`. A hostile repo never silently enables execution or network, and never receives a harness secret. |
| **Secrets** | MCP server API keys / tokens | **Global trusted layer only** (`~/.kiri/credentials.json`, `0600`). The project layer never supplies a secret. |

This extends ADR 0012's "project contributes only `effort`" rule: the project *may* define passive content and
the *metadata* of an active capability, but active capabilities from the project start **disabled** and require
an explicit gate. The gate reuses the existing onboarding/approval machinery (a first-run prompt carried as a
`BootNotice`).

### Storage locations

Two layers, both under `.kiri/` (consistent with project memory, which already lives in `<workspace>/.kiri/memory/`):

```
~/.kiri/{rules,commands,skills,agents,hooks,mcp}/      # global (trusted)
<workspace>/.kiri/{rules,commands,commands,...}/       # project (untrusted — load passive, gate active)
```

Each resource is a Markdown file with optional YAML frontmatter. Global loads first, project after; a project
entry with the same `id`/`name` extends or overrides the global one (never silently replaces a global one for
routing-relevant fields).

Discoverability from foreign ecosystems (`.claude/`, `.cursor/`) is deliberate future work — recorded here so
the loader has one discovery layer per resource type.

### Ordering in the system prompt

Passive content is injected before `# Security`, so the harness's security policy is always final:

```
# Memory & preferences
# Rules                       <- {RULES}  (always-on rules; absent when none)
# Skills                      <- {SKILLS} (one-line index; bodies fetched on demand via use_skill)
# User Instructions           <- {INSTRUCTIONS} (ADR 0019)
# Security                    <- always last
```

### Composition root

`app::wire` is the only place the extension adapters are chosen and the catalogs assembled, injected as ports.
A new `extensions` bounded context holds the discovery + loading + the gate state; a `mcp` context and a
`hooks` context own network/process I/O (the sanctioned sites for those, mirroring `provider`/`sync`).

### Trust gate implementation: TOFU (Trust On First Use)

`domain::gate::resolve(layer, previously_approved)` is the pure decision (global always `Approved`; project
`Approved` only when `previously_approved`). The "previously approved" bit comes from
`infrastructure::trust_store::ExtensionsTrustStore`, a `0600` JSON file at `~/.kiri/extensions_trust.json`
(mirroring `FileSecretStore`'s adapter shape: read-modify-write, crash-atomic, owner-only), keyed by
`capability id -> approved content hash` (`domain::gate::content_hash`, blake3, 16 hex chars).

TOFU semantics: approve a capability once, and it stays approved as long as its content is unchanged. If a
hostile repo edits an already-approved hook/MCP config after approval, its hash no longer matches the stored
one — the gate reports `Pending` again on the next boot, exactly as if it had never been approved. Revoking
an approval today means deleting its entry from the trust-store file by hand; a `/trust` management command
is future work.

This lands ahead of any real active-capability type (hooks/MCP, Fase 4/5) as infrastructure only — nothing
calls `resolve`/`ExtensionsTrustStore` in production yet, since there is nothing to gate. The first hook
discovered from a project layer is the first real caller.

## Consequences

- A team commits `.kiri/rules/` and `.kiri/commands/` to share behavioural guidance and slash commands;
  both take effect on the next session start (the prompt is rendered once at boot).
- Active capabilities (hooks/MCP) committed to a repo never auto-execute on `cd`; the user approves them once.
- An architecture guard (to be added with the `mcp`/`hooks` contexts) will keep network/process I/O confined to
  those contexts' `infrastructure/` layer, mirroring the domain-purity guard.
- The `extensions` context's `domain/` layer stays pure (frontmatter parsing, resource types); filesystem
  discovery lives in `infrastructure/`, like the `memory` and `sync` contexts' own data dirs.
