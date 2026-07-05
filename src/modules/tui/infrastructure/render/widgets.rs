pub mod approval;
pub mod command_menu;
pub mod editor;
pub mod header;
pub mod hint_line;
pub mod meta_rule;
pub mod picker;
pub mod plan_pane;
pub mod search_bar;
pub mod selection_overlay;
pub mod sidebar;
pub mod splash;
pub mod transcript_pane;
pub mod wizard;

use ratatui::style::Style;

use crate::modules::tui::infrastructure::theme;

/// The selected-row marker idiom shared by every single-choice list (approval, plan, picker, command
/// menu, wizard): the highlighted row carries the `❯` caret in the lone accent colour; the rest get a
/// blank gutter in dim. One source so the lists read identically.
pub fn option_marker(selected: bool) -> (&'static str, Style) {
    if selected {
        ("❯ ", theme::accent())
    } else {
        ("  ", theme::dim())
    }
}
