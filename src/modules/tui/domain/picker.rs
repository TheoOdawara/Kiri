use crate::modules::tui::domain::nav::wrapping_step;

/// Which setting a generic picker chooses, so the keymap maps the highlighted row to the right effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Models,
    Effort,
    Provider,
    Sessions,
}

/// A generic single-choice picker modal (used by `/models` and `/effort`), rendered with the same
/// borderless stanza as the approval/plan boxes. `options` are the selectable labels in display order;
/// `selected` indexes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub kind: PickerKind,
    pub label: String,
    pub action: String,
    pub options: Vec<String>,
    pub selected: usize,
}

impl Picker {
    /// Open a picker, clamping `selected` into range (or 0 when there are no options).
    pub fn new(
        kind: PickerKind,
        label: impl Into<String>,
        action: impl Into<String>,
        options: Vec<String>,
        selected: usize,
    ) -> Self {
        let selected = selected.min(options.len().saturating_sub(1));
        Self {
            kind,
            label: label.into(),
            action: action.into(),
            options,
            selected,
        }
    }

    /// Move the highlight by `delta` rows, wrapping within the options.
    pub fn move_cursor(&mut self, delta: i32) {
        self.selected = wrapping_step(self.selected, delta, self.options.len());
    }
}
