use ratatui::style::{Color, Style};

/// The "Tamahagane Void" brand palette (truecolor). Deliberately drops the old ANSI-16 fallback: the
/// goal is brand fidelity on modern truecolor terminals — deep steel-on-void, gates in sharp accents.
///
/// The one law: *weight is brightness, colour is spent like gold, heat means work, everything cools to
/// steel*. Chrome emphasis is a brighter step on the steel ramp, never `Modifier::BOLD`; an accent
/// (HIGHLIGHT) marks exactly one live thing at a time. Markdown bold/italic inside an answer is content
/// the model wrote — that keeps its modifiers; only the chrome obeys the brightness law.
pub const VOID: Color = Color::Rgb(0x0D, 0x11, 0x17); // #0D1117 — base background
pub const STEEL: Color = Color::Rgb(0xE6, 0xED, 0xF3); // #E6EDF3 — default text (polished steel)
pub const BRAND: Color = Color::Rgb(0x8B, 0x94, 0x9E); // #8B949E — rules, delimiters, dim, idle gate
pub const SUCCESS: Color = Color::Rgb(0x3F, 0xB9, 0x50); // #3FB950 — green gate — passed / [OK]
pub const WARNING: Color = Color::Rgb(0xD2, 0x99, 0x22); // #D29922 — yellow gate — info / approval pending
pub const ERROR: Color = Color::Rgb(0xF8, 0x51, 0x49); // #F85149 — red gate — error / blade cut
pub const HIGHLIGHT: Color = Color::Rgb(0x58, 0xA6, 0xFF); // #58A6FF — cyan action — the single live accent
pub const CODE_FG: Color = Color::Rgb(0xF0, 0x88, 0x3E); // #F0883E — orange — inline code / fresh-struck line
pub const CODE_BG: Color = Color::Rgb(0x1C, 0x22, 0x2B); // #1C222B — code block background
pub const HEADING: Color = Color::Rgb(0x6F, 0xB0, 0xCC); // #6FB0CC — soft cyan — headings / `◆ kiri` label (whispered)

/// The steel brightness ramp — emphasis is a brighter step, recession a darker one; this replaces every
/// chrome `Modifier::BOLD`. Index 0 is full weight ("bold"), 4 is the most receded (vignette / hairline).
pub const STEEL_RAMP: [Color; 5] = [
    STEEL,                        // 0 — full weight
    Color::Rgb(0xC2, 0xCA, 0xD4), // 1
    Color::Rgb(0x9B, 0xA4, 0xAF), // 2
    BRAND,                        // 3 — dim / secondary / rules
    Color::Rgb(0x5A, 0x63, 0x6D), // 4 — receded / vignette
];

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

/// Chrome emphasis under the brightness law: full-weight steel, no bold — the "strong" of the UI.
pub fn strong() -> Style {
    Style::default().fg(STEEL_RAMP[0])
}

/// The single live accent (cyan). Marks exactly one working thing per frame; never bold (here weight is
/// colour, not stroke).
pub fn accent() -> Style {
    Style::default().fg(HIGHLIGHT)
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

/// Map a gate state to its prompt glyph and style. Weight is brightness: the typing gate is the brightest
/// steel step, not bold; the busy/approval/error gates each spend their one accent/warn/error colour in
/// this single cell.
pub fn gate(state: GateState) -> (char, Style) {
    match state {
        GateState::Idle => ('⬡', Style::default().fg(BRAND)),
        GateState::Typing => ('⬢', Style::default().fg(STEEL_RAMP[0])),
        GateState::Busy(frame) => (
            SPINNER[frame % SPINNER.len()],
            Style::default().fg(HIGHLIGHT),
        ),
        GateState::Approval => ('⬢', Style::default().fg(WARNING)),
        GateState::Error => ('⬢', Style::default().fg(ERROR)),
    }
}
