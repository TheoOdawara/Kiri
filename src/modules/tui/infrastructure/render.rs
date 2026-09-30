//! The render path: the view tree, layout geometry, the markdown renderer, text helpers, the theme, and
//! the widgets. Grouped under `render/`; the infrastructure root re-exports them so the pre-grouping
//! `tui::infrastructure::{view,layout,markdown,text,theme,widgets}` paths keep resolving.

pub mod layout;
pub mod markdown;
pub mod text;
pub mod theme;
pub mod view;
pub mod widgets;
