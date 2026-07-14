//! Renders the add-provider wizard into its reserved region above the input. The `Kind` step lists the
//! provider kinds as selectable rows; the text steps render a real `InputBuffer` (soft-wrap + cursor).
//! The API-key step keeps the value masked. Borderless, matching the approval/plan stanzas.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::editor;
use crate::modules::tui::domain::wizard::{ProviderWizard, WIZARD_KINDS, WizardStep};
use crate::modules::tui::infrastructure::theme;
use crate::shared::kernel::provider::ProviderKind;

/// Cap field height so a huge paste never eats the whole frame.
const MAX_FIELD_LINES: usize = 6;
/// Columns taken by the ` › ` field gutter.
const FIELD_GUTTER: u16 = 3;

/// The rows the wizard occupies in `area`: a header, a blank, the body, a blank, and the hint — used by
/// the view to reserve exactly its height.
pub fn box_dims(area: Rect, wizard: &ProviderWizard) -> (u16, u16) {
    let body = match wizard.step {
        WizardStep::Kind => 1 + WIZARD_KINDS.len(),
        WizardStep::Thinking => 3, // prompt + "Sim" + "Não"
        _ => {
            let field_w = area.width.saturating_sub(FIELD_GUTTER).max(1) as usize;
            let field_lines =
                editor::wrapped_line_count(&wizard.draft, field_w).clamp(1, MAX_FIELD_LINES);
            1 + field_lines // prompt + field
        }
    };
    // header + blank + body + blank + hint
    let height = ((3 + body + 1) as u16).min(area.height.max(1));
    (area.width, height)
}

pub fn render(wizard: &ProviderWizard, frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);

    let title = if wizard.onboarding {
        "Bem-vindo ao Kiri"
    } else if wizard.edit_mode {
        "editar provider"
    } else {
        "novo provider"
    };

    if wizard.step == WizardStep::Kind || wizard.step == WizardStep::Thinking {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::styled(
            format!(" {} · {}", title, step_label(wizard.step)),
            theme::dim(),
        ));
        lines.push(Line::default());

        if wizard.step == WizardStep::Kind {
            let prompt = if wizard.onboarding {
                " Escolha seu provider para começar:"
            } else {
                " Escolha o tipo de provider:"
            };
            lines.push(Line::styled(prompt, theme::strong()));
            for (i, kind) in WIZARD_KINDS.iter().enumerate() {
                let (marker, style) = super::option_marker(i == wizard.kind_selected);
                lines.push(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled(kind_label(*kind), style),
                ]));
            }
        } else {
            lines.push(Line::styled(
                " Ativar raciocínio estendido (thinking) para este provider?",
                theme::strong(),
            ));
            for (i, label) in ["Sim", "Não"].iter().enumerate() {
                let selected = wizard.thinking_selected_index() == i;
                let (marker, style) = super::option_marker(selected);
                lines.push(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled(*label, style),
                ]));
            }
        }

        lines.push(Line::default());
        lines.push(Line::styled(
            " Enter: próximo · Esc: cancelar",
            theme::dim(),
        ));
        frame.render_widget(Paragraph::new(lines).style(theme::base()), area);
        return;
    }

    // Text step: header + prompt + live field (wrap + cursor) + hint.
    let field_w = area.width.saturating_sub(FIELD_GUTTER).max(1) as usize;
    let field_lines =
        editor::wrapped_line_count(&wizard.draft, field_w).clamp(1, MAX_FIELD_LINES) as u16;

    let [header, blank1, prompt_row, field_row, blank2, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(field_lines),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(" {} · {}", title, step_label(wizard.step)),
            theme::dim(),
        )),
        header,
    );
    frame.render_widget(Paragraph::new(Line::default()), blank1);
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(" {}", wizard_prompt(wizard)),
            theme::strong(),
        )),
        prompt_row,
    );

    let [gutter, field_area] =
        Layout::horizontal([Constraint::Length(FIELD_GUTTER), Constraint::Min(1)]).areas(field_row);
    frame.render_widget(Paragraph::new(Line::styled(" › ", theme::accent())), gutter);

    if wizard.step == WizardStep::ApiKey {
        // Never paint the raw key: mask to bullets and place a caret at the logical cursor column.
        let n = wizard.draft.text().chars().count();
        let masked: String = "•".repeat(n);
        let (_, col) = wizard.draft.cursor();
        let col = col.min(n);
        let mut spans = Vec::new();
        if col > 0 {
            spans.push(Span::styled(
                masked.chars().take(col).collect::<String>(),
                theme::base(),
            ));
        }
        spans.push(Span::styled("▏", theme::accent()));
        if col < n {
            spans.push(Span::styled(
                masked.chars().skip(col).collect::<String>(),
                theme::base(),
            ));
        } else if n == 0 {
            // Empty field: caret alone (already pushed).
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(theme::base()),
            field_area,
        );
    } else {
        frame.render_widget(wizard.draft.widget(), field_area);
    }

    frame.render_widget(Paragraph::new(Line::default()), blank2);
    frame.render_widget(
        Paragraph::new(Line::styled(
            " Enter: próximo · Esc: cancelar",
            theme::dim(),
        )),
        hint,
    );
}

fn step_label(step: WizardStep) -> &'static str {
    // No fixed fraction: the `id` step is shown only for keyless-capable kinds, so the total varies.
    match step {
        WizardStep::Kind => "tipo",
        WizardStep::ProviderId => "id",
        WizardStep::BaseUrl => "endpoint",
        WizardStep::Model => "modelo",
        WizardStep::ExtraModels => "modelos extras",
        WizardStep::Thinking => "thinking",
        WizardStep::ApiKey => "chave",
    }
}

/// The prompt for the current text step, kind-aware: the API-key prompt advertises that the key is
/// optional for keyless-capable kinds (Ollama / LM Studio), or that a blank key keeps the existing one
/// in edit mode.
fn wizard_prompt(wizard: &ProviderWizard) -> String {
    match wizard.step {
        WizardStep::Kind | WizardStep::Thinking => String::new(),
        WizardStep::ProviderId => "Identificador (ex.: lmstudio, openrouter):".to_string(),
        // Keyless-capable kinds are the OpenAI-compatible local servers (LM Studio / Ollama), whose base
        // URL must carry `/v1` — the adapter only appends `chat/completions`. Vendor kinds keep the plain
        // prompt (their canonical base URL already includes any version segment).
        WizardStep::BaseUrl if !wizard.key_required() => {
            "Base URL (inclua /v1, ex.: http://host:1234/v1):".to_string()
        }
        WizardStep::BaseUrl => "Base URL:".to_string(),
        WizardStep::Model => "Modelo default:".to_string(),
        WizardStep::ExtraModels => "Modelos extras (separados por vírgula, opcional):".to_string(),
        WizardStep::ApiKey if wizard.edit_mode => {
            "API key (vazio = manter chave atual):".to_string()
        }
        WizardStep::ApiKey if !wizard.key_required() => {
            "API key (opcional — vazia p/ Ollama / LM Studio):".to_string()
        }
        WizardStep::ApiKey => "API key:".to_string(),
    }
}

fn kind_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Nvidia => "NVIDIA",
        ProviderKind::Openai => "GPT (OpenAI) — API key",
        ProviderKind::Anthropic => "Claude (Anthropic) — API key",
        ProviderKind::OpenAiCompatible => "OpenAI-compatible — endpoint próprio",
        ProviderKind::Custom => "Custom — endpoint arbitrário",
    }
}
