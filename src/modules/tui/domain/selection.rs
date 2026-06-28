/// How a screen selection grows from a click: a plain drag selects by character; a double/triple click
/// selects the word/line under the cursor. The actual character ranges for `Word`/`Line` are derived
/// from the rendered buffer (only the overlay/runtime can see the glyphs), so the reducer only tags the
/// intent here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Granularity {
    #[default]
    Char,
    Word,
    Line,
}

/// What the selection is waiting for the runtime to do on the next draw. The copy must happen in the
/// runtime (it scrapes the rendered buffer), so the reducer can only request it: `CopyAndKeep` (mouse
/// release — leave the highlight up) or `CopyAndClear` (Ctrl+C — drop it after, so the next Ctrl+C is
/// free to cancel/quit again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionState {
    #[default]
    Idle,
    CopyAndKeep,
    CopyAndClear,
}

/// A text selection over the rendered screen, in absolute terminal cells. It lives in screen space (not
/// source text), so it works uniformly over the transcript, tool output, and the composer. The reducer
/// sets `anchor`/`head`/`granularity`/`state`; the overlay paints it and the runtime scrapes the cells
/// to copy. `Copy` so the runtime can lift it out of the model without holding a borrow across a draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenSelection {
    /// Where the gesture began (fixed end).
    pub anchor: (u16, u16),
    /// The moving end (follows the drag / last click).
    pub head: (u16, u16),
    pub granularity: Granularity,
    pub state: SelectionState,
}

impl ScreenSelection {
    pub fn new(col: u16, row: u16, granularity: Granularity) -> Self {
        Self {
            anchor: (col, row),
            head: (col, row),
            granularity,
            state: SelectionState::Idle,
        }
    }

    /// Move the head; the anchor stays put.
    pub fn extend(&mut self, col: u16, row: u16) {
        self.head = (col, row);
    }

    /// A character selection collapses to nothing when anchor == head (a bare click). A word/line
    /// selection is never empty — even a single click expands to the word/line under it.
    pub fn is_empty(&self) -> bool {
        self.granularity == Granularity::Char && self.anchor == self.head
    }

    /// `(start, end)` ordered by row then column, so the overlay never special-cases drag direction.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let key = |(c, r): (u16, u16)| (r, c);
        if key(self.anchor) <= key(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_selection_is_empty_only_for_a_char_click() {
        // A bare char click (anchor == head) selects nothing; a word/line click or any drag does not.
        assert!(ScreenSelection::new(3, 2, Granularity::Char).is_empty());
        assert!(!ScreenSelection::new(3, 2, Granularity::Word).is_empty());
        assert!(!ScreenSelection::new(3, 2, Granularity::Line).is_empty());
        let mut s = ScreenSelection::new(3, 2, Granularity::Char);
        s.extend(4, 2);
        assert!(!s.is_empty());
    }
}
