/// Transcript scroll position, measured as lines scrolled up from the newest content. Zero means
/// pinned to the bottom (auto-following new output). The view clamps it to the available scrollback.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    pub scrollback: u16,
}

impl Scroll {
    pub fn up(&mut self, lines: u16) {
        self.scrollback = self.scrollback.saturating_add(lines);
    }

    pub fn down(&mut self, lines: u16) {
        self.scrollback = self.scrollback.saturating_sub(lines);
    }

    pub fn pin(&mut self) {
        self.scrollback = 0;
    }

    /// Jump to the top of the scrollback. The view clamps to the available history, so saturating to
    /// the maximum is enough — no viewport height needs to leak into the model.
    pub fn top(&mut self) {
        self.scrollback = u16::MAX;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_top_saturates_and_pin_resets() {
        let mut s = Scroll::default();
        s.top();
        assert_eq!(s.scrollback, u16::MAX);
        s.pin();
        assert_eq!(s.scrollback, 0);
    }
}
