pub mod render;
pub mod runtime;

// The render path lives under `render/`; re-export its modules at the infrastructure root so the
// pre-grouping `tui::infrastructure::{view,layout,markdown,text,theme,widgets}` paths keep resolving.
pub use render::{layout, markdown, text, theme, view, widgets};
// The IO spine (bridge/input/clipboard/terminal_guard) lives under `runtime/` and is referenced locally
// within `runtime/` (self/super); it has no production consumer outside `runtime`, so it needs no root
// re-export.
