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
pub const GUILLOCHE: Color = Color::Rgb(0x30, 0x36, 0x3D); // #30363D — etched hairline above the approval stanza
pub const TEMPER_BLUE: Color = Color::Rgb(0x6C, 0xB6, 0xFF); // #6CB6FF — one-shot gate quench on turn-settle only

/// The steel brightness ramp — emphasis is a brighter step, recession a darker one; this replaces every
/// chrome `Modifier::BOLD`. Index 0 is full weight ("bold"), 4 is the most receded (vignette / hairline).
pub const STEEL_RAMP: [Color; 5] = [
    STEEL,                        // 0 — full weight
    Color::Rgb(0xC2, 0xCA, 0xD4), // 1
    Color::Rgb(0x9B, 0xA4, 0xAF), // 2
    BRAND,                        // 3 — dim / secondary / rules
    Color::Rgb(0x5A, 0x63, 0x6D), // 4 — receded / vignette
];

/// The cooling ramp — a freshly-landed answer line settles from forge-warm down to polished steel. The
/// signature reveal lerps a line's foreground along this over its first ~150 ms, then it is steel forever.
pub const COOLING_RAMP: [Color; 3] = [
    CODE_FG,                      // forge-warm, just struck
    Color::Rgb(0xC9, 0x9A, 0x6E), // cooling
    STEEL,                        // polished steel
];

/// The temper-quench ramp — the one-shot reward beat the gate gives when a turn settles: the busy cyan
/// quenches through temper-blue and cools into the resting idle colour, so it lands seamlessly on the
/// idle gate with no jump.
pub const QUENCH_RAMP: [Color; 3] = [HIGHLIGHT, TEMPER_BLUE, BRAND];

/// The 10-frame braille spinner.
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The spinner glyph for animation `frame`, wrapping the index into [`SPINNER`]. One source so the gate
/// prompt and the meta rule never index the array differently.
pub fn spinner_glyph(frame: usize) -> char {
    SPINNER[frame % SPINNER.len()]
}

/// Linearly interpolate a colour along ordered `stops` by `t` in `[0, 1]`. Pure (no clock, no I/O) so it
/// is unit-testable like `spinner_frame`; `t <= 0` returns the first stop, `t >= 1` the last. Stops must
/// be `Color::Rgb` (the whole palette is); any other variant is treated as black.
pub fn ramp(stops: &[Color], t: f32) -> Color {
    match stops {
        [] => STEEL,
        [only] => *only,
        _ => {
            let t = t.clamp(0.0, 1.0);
            let segments = (stops.len() - 1) as f32;
            let scaled = t * segments;
            let idx = (scaled.floor() as usize).min(stops.len() - 2);
            lerp_rgb(stops[idx], stops[idx + 1], scaled - idx as f32)
        }
    }
}

fn rgb_parts(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

fn lerp_rgb(from: Color, to: Color, t: f32) -> Color {
    let (fr, fg, fb) = rgb_parts(from);
    let (tr, tg, tb) = rgb_parts(to);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::Rgb(mix(fr, tr), mix(fg, tg), mix(fb, tb))
}

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

/// The highlight for an in-app screen text selection — void-on-brand, the same inversion the editor uses
/// for its own selection, so a dragged highlight reads consistently across the whole UI without claiming
/// the single live accent (cyan).
pub fn selection() -> Style {
    Style::default().fg(VOID).bg(BRAND)
}

/// The input prompt is a live Quality Gate: its glyph and color encode the editor's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        GateState::Busy(frame) => (spinner_glyph(frame), Style::default().fg(HIGHLIGHT)),
        GateState::Approval => ('⬢', Style::default().fg(WARNING)),
        GateState::Error => ('⬢', Style::default().fg(ERROR)),
    }
}
