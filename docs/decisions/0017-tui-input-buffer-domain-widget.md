# ADR 0017 — `InputBuffer` may own a `tui_textarea::TextArea` in `tui/domain`

- Status: Accepted
- Date: 2026-06-27

## Context

ADR 0003 sets the modular-hexagonal invariant: `domain/` holds **pure data and rules, no I/O and no
framework dependencies**. Across the codebase exactly one domain file breaks the letter of that rule —
`src/modules/tui/domain/view_state.rs` imports `ratatui::style::Style` and `tui_textarea::{TextArea, …}`,
and its `InputBuffer` embeds a stateful `TextArea<'static>` (cursor, viewport, soft word-wrap, selection,
undo/redo) and exposes `set_styles`/`widget` over `ratatui` types.

This is a **deliberate** tradeoff, but it was implicit. The Elm-style reducer needs the editor's cursor /
selection / undo state to be the model's single source of truth, and the editing behaviour (multi-line
soft-wrapping, word motion, undo/redo) is exactly what `tui_textarea` provides. Reimplementing that as a
pure-data shadow would duplicate the widget's internal state and the wrapping/undo logic — speculative
re-work with no present benefit, and a correctness risk (two editors that can disagree). The widget is
already firewalled: it lives in one file behind a thin wrapper that exposes only accessors, and all
side effects (clipboard read, PNG encode, rendering) stay in `tui/infrastructure`.

The working contract requires architecture deviations to be **ratified, not left silent**. This ADR
ratifies the coupling and makes its boundary enforceable.

## Decision

`InputBuffer` may own a `tui_textarea::TextArea` inside `tui/domain` as the editor's **authoritative
state**. This single file is the **only** sanctioned `domain → UI-crate` coupling in the codebase:

- It exposes **pure accessors** over the widget (`text`, cursor position, `widget()` for rendering); the
  reducer stays pure.
- All clipboard and render **side effects** remain in `tui/infrastructure` — the domain wrapper performs
  none.
- **No other domain file** — in any module, at any nesting depth under `src/modules/*/domain/` — may
  import `ratatui` or `tui_textarea`. This is an enforced boundary, not a convention.

A guard test (`only_input_buffer_couples_domain_to_ui_crates`) walks **every** `*.rs` recursively under
each `src/modules/*/domain/` directory and asserts none except `InputBuffer`'s home contains
`use ratatui` / `use tui_textarea`. So a future nested `domain/<sub>/foo.rs` cannot silently re-breach the
rule, and a reviewer sees the boundary fail fast.

## Consequences

- The one accepted exception to ADR 0003's "domain has no framework dependency" is now explicit, scoped to
  a single file, and machine-enforced — a future domain file importing a UI crate fails the guard test.
- When Wave 3 (TUIC-02) splits `InputBuffer` into its own `tui/domain/input_buffer.rs`, the guard test's
  allow-list must be updated to point at the new path (the exception moves with the type, it does not grow).
- No production behaviour changes; this is documentation plus one structural test.

Amends ADR 0003 (modular-hexagonal architecture) as its single sanctioned `domain ↔ UI-crate` exception.
