# ADR 0006 — Rich input editor (tui-textarea), OS clipboard, and multimodal user messages

- Status: Accepted
- Date: 2026-06-23

## Context

The TUI's input was a hand-rolled `InputBuffer` (text + a byte cursor) — deliberately minimal per
*native-over-deps*. It covered arrow keys, Home/End, Backspace/Delete and bracketed-paste of **text**,
but nothing a real editor offers: no selection, no copy/cut to the OS clipboard, no word-wise motion, no
undo/redo, and no way to paste an **image**. The user asked to "write as in a text editor, with all the
shortcuts, copy & paste of image and text".

Two extra UX fixes shipped alongside (no ADR needed, just code): the live "thinking" line glued above the
prompt was removed, and the plan/approval confirmation box was given a dedicated layout region directly
above the input so it is always anchored to the bottom instead of being carved out of the scrolling
transcript.

Reaching a full editor by extending the hand-rolled buffer would mean re-implementing selection across
soft-wrapped lines, Unicode word boundaries, an undo/redo history, and clipboard plumbing — a large,
bug-prone surface. The model the user runs (`nvidia/nemotron-3-super-120b`) is text-only, but the
OpenAI-compatible wire format already defines multimodal `content` parts, so image support is a
serialization concern plus a clipboard read.

## Decision

Deviate from *native-over-deps* for the editor and clipboard, and extend the conversation domain to carry
images on user messages.

### Adopt `tui-textarea` for the editor

`Model.input` stays a thin domain wrapper (`InputBuffer` in `tui/domain/view_state.rs`) that now holds a
`tui_textarea::TextArea` (crate `tui-textarea-2 0.11`, the fork tracking `ratatui 0.30`; its lib name is
`tui_textarea`). The wrapper confines the widget type to one module and exposes only what the reducer and
renderer need: `text`/`set`/`take`/`is_empty`, `feed(Input)` (the single path for ordinary editing),
`select_all`/`undo`/`redo`/`copy_selection`/`cut_selection`/`is_selecting`, `cursor_row`/`last_row` (for
history-at-edge), `set_styles`, and `widget()`.

- The keymap (`application`) stays the gatekeeper: it intercepts app/clipboard chords and forwards
  everything else to the widget via `feed`. Windows conventions override the widget's emacs defaults —
  **Ctrl+A** select-all, **Ctrl+C/X/V** copy/cut/paste, **Ctrl+Z/Y** undo/redo — while selection
  (Shift+motion), word motion (Ctrl+arrows), and multi-line navigation come from the widget for free.
- Up/Down recall history only at the buffer's first/last row; inside a multi-line buffer they move the
  cursor. Transcript scroll moved to PageUp/PageDown (+ Shift for a page) and Ctrl+Home/End.
- **Trade-off:** wrapping changes from the old greedy soft-wrap to the widget's `WordOrGlyph` soft-wrap
  (still wraps; long words hard-split). The renderer keeps a lightweight row count for sizing the input
  box; if it is ever a row short the widget scrolls internally.

**Accepted layering deviation (domain purity).** `InputBuffer` lives in `tui/domain/view_state.rs` yet now
holds a stateful presentation widget (`TextArea` carries its own undo history, yank buffer, and viewport),
and the file imports `ratatui::style::Style` and `tui_textarea` types. This bends the CLAUDE.md invariant
"`domain` = pure data/rules". We accept it deliberately: the `TextArea` performs **no I/O** (the invariant's
hard rule — network/filesystem/stdout — is intact; clipboard I/O stays in infrastructure and the reducer
stays pure), and the alternative of moving the editor into `infrastructure` would force the
application-tier reducer (`keymap`/`update`) to depend on an infrastructure type to drive editing — a worse
hexagonal inversion than a self-contained value held in `domain`. Consequences of the choice: the
application layer imports `tui_textarea::{Input, Key}` as inert value types to call the wrapper's
`feed(Input)`, and `InputBuffer` dropped its `PartialEq/Eq` derives (the widget is not comparable; nothing
compared input buffers by value).

### OS clipboard via `arboard` (+ `png`, `base64`)

A new `tui/infrastructure/clipboard.rs` adapter wraps `arboard` (feature `image-data`). It reads the
clipboard preferring an image (encoded RGBA8 → PNG via `png`, then base64 → `data:image/png;base64,...`)
over text, and copies text. All failures collapse to a no-op — clipboard access never crashes the TUI.

Clipboard is **device I/O**, so it lives in `tui/infrastructure`, not in the pure reducer. The reducer
emits effects (`CopyToClipboard`, `PasteClipboard`) and the runtime performs them, then routes the result
back as a message (`Msg::Paste` / `Msg::ImageAttached`). Because terminals (Windows Terminal, VS Code)
bind Ctrl+V to their own paste and can swallow it — and never deliver an image — a `/paste` command
provides a reliable, discoverable path to grab the clipboard image; Ctrl+V also works where the terminal
forwards it.

### Multimodal user messages

`agent/domain/message.rs` gains `images: Vec<String>` (data URLs) and a `Message::user_multimodal`
constructor; all other constructors keep `images` empty. The provider DTO
(`provider/infrastructure/openai/message_dto.rs`) serializes `content` as a plain string when there are
no images (the common path, byte-for-byte unchanged) or as an `[{type:text}, {type:image_url}...]` parts
array when a user message carries images. The runtime builds `user_multimodal` only when the submitted
prompt has staged attachments.

> Capability note: images are only *useful* with a vision-capable model. The code does not gate on model
> name; it just sends the multimodal content. Point `NVIDIA_MODEL` at a vision model to use it.

## Consequences

- Four new dependencies: `tui-textarea-2`, `arboard`, `png`, `base64`. `unsafe_code = "forbid"` is
  unaffected (it applies to our crate only).
- The editor's behaviour is now broad and idiomatic; the wrapper keeps the widget out of the rest of the
  codebase, so a future swap is localized.
- The DTO change is additive and backward-compatible: existing text-only serialization is unchanged,
  guarded by tests.
