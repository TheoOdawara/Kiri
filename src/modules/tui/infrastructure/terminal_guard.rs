use crossterm::event::DisableBracketedPaste;

/// Restores the terminal on every exit path. `ratatui::init` installs a panic hook that restores on
/// panic; this guard covers normal returns and `?`-propagated errors, and additionally disables
/// bracketed paste (which we enable explicitly after init, since `ratatui::init` does not).
pub struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
        ratatui::restore();
    }
}
