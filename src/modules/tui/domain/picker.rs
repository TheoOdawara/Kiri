use crate::modules::tui::domain::nav::wrapping_step;

/// Which setting a generic picker chooses, so the keymap maps the highlighted row to the right effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerKind {
    Models,
    Effort,
    Provider,
    Sessions,
    /// The action sub-menu for a specific provider: Ativar / Editar / Remover.
    ProviderAction(String),
    /// The delete-confirmation picker for a provider: Sim, remover / Cancelar.
    ProviderDeleteConfirm(String),
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
    pub query: String,
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
            query: String::new(),
        }
    }

    /// The subset of options that match the query (case-insensitive substring match).
    /// Returns a list of `(original_index, option_text)`.
    pub fn filtered_options(&self) -> Vec<(usize, &str)> {
        let q = self.query.to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter(|(_, opt)| opt.to_lowercase().contains(&q))
            .map(|(idx, opt)| (idx, opt.as_str()))
            .collect()
    }

    /// Move the highlight by `delta` rows, wrapping within the filtered options.
    pub fn move_cursor(&mut self, delta: i32) {
        let count = self.filtered_options().len();
        if count > 0 {
            self.selected = wrapping_step(self.selected, delta, count);
        } else {
            self.selected = 0;
        }
    }
}
