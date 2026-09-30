use crossterm::event::{DisableBracketedPaste, DisableMouseCapture};

/// Restores the terminal on every exit path. `ratatui::init` installs a panic hook that restores on
/// panic; this guard covers normal returns and `?`-propagated errors, and additionally disables
/// bracketed paste and mouse capture (which we enable explicitly after init, since `ratatui::init`
/// does neither).
pub struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // In `Drop` there is nowhere to propagate an error and no recovery on the exit path; restoring
        // the terminal is inherently best-effort, so the Result is deliberately ignored.
        let _ = crossterm::execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture
        );
        ratatui::restore();
    }
}
