use ratatui::style::{Color, Modifier, Style};

/// A small native palette built on ANSI-16 named colors only: nothing to degrade on terminals without
/// truecolor, and no theme-engine machinery.
pub const ACCENT: Color = Color::Magenta;
pub const USER: Color = Color::Cyan;
pub const NOTICE: Color = Color::Yellow;
pub const ERROR: Color = Color::Red;

/// The 10-frame braille spinner, matching the plain terminal's animation.
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn accent() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}
