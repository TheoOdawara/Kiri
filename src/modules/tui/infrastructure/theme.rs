use ratatui::style::{Color, Modifier, Style};

/// The "Tamahagane Void" brand palette (truecolor). Deliberately drops the old ANSI-16 fallback: the
/// goal is brand fidelity on modern truecolor terminals — deep steel-on-void, gates in sharp accents.
pub const VOID: Color = Color::Rgb(0x0D, 0x11, 0x17); // #0D1117 — base background
pub const STEEL: Color = Color::Rgb(0xE6, 0xED, 0xF3); // #E6EDF3 — default text (polished steel)
pub const BRAND: Color = Color::Rgb(0x8B, 0x94, 0x9E); // #8B949E — rules, delimiters, dim, idle gate
pub const SUCCESS: Color = Color::Rgb(0x3F, 0xB9, 0x50); // #3FB950 — green gate — passed / [OK]
pub const WARNING: Color = Color::Rgb(0xD2, 0x99, 0x22); // #D29922 — yellow gate — info / approval pending
pub const ERROR: Color = Color::Rgb(0xF8, 0x51, 0x49); // #F85149 — red gate — error / blade cut
pub const HIGHLIGHT: Color = Color::Rgb(0x58, 0xA6, 0xFF); // #58A6FF — cyan action — active input / loading
pub const CODE_FG: Color = Color::Rgb(0xF0, 0x88, 0x3E); // #F0883E — orange — inline code / fenced code
pub const CODE_BG: Color = Color::Rgb(0x1C, 0x22, 0x2B); // #1C222B — code block background
pub const HEADING: Color = Color::Rgb(0x7D, 0xC9, 0xE8); // #7DC9E8 — soft cyan — headings

/// The 10-frame braille spinner.
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The root style — steel on the void — painted across the whole frame so every cell inherits it.
pub fn base() -> Style {
    Style::default().fg(STEEL).bg(VOID)
}

/// Neutral/secondary text: rules, delimiters, reasoning, hints.
pub fn dim() -> Style {
    Style::default().fg(BRAND)
}

/// Active/loading accent (cyan), bold.
pub fn accent() -> Style {
    Style::default().fg(HIGHLIGHT).add_modifier(Modifier::BOLD)
}

/// The input prompt is a live Quality Gate: its glyph and color encode the editor's state.
pub enum GateState {
    /// Active, empty buffer — the gate is open and waiting.
    Idle,
    /// Active, the user is composing.
    Typing,
    /// A turn is running; carries the spinner frame.
    Busy(usize),
    /// A tool-call confirmation is awaiting an answer.
    Approval,
    /// The last transcript line was an error — the blade cut.
    Error,
}

/// Map a gate state to its prompt glyph and style.
pub fn gate(state: GateState) -> (char, Style) {
    match state {
        GateState::Idle => ('⬡', Style::default().fg(BRAND)),
        GateState::Typing => (
            '⬢',
            Style::default().fg(HIGHLIGHT).add_modifier(Modifier::BOLD),
        ),
        GateState::Busy(frame) => (
            SPINNER[frame % SPINNER.len()],
            Style::default().fg(HIGHLIGHT).add_modifier(Modifier::BOLD),
        ),
        GateState::Approval => (
            '⬢',
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
        ),
        GateState::Error => ('⬢', Style::default().fg(ERROR).add_modifier(Modifier::BOLD)),
    }
}
