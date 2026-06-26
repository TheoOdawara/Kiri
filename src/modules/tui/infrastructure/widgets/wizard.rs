//! Renders the add-provider wizard into its reserved region above the input. The `Kind` step lists the
//! provider kinds as selectable rows; the text steps show the current field's value (the API key
//! masked). Borderless, matching the approval/plan stanzas — containment is positional + perceptual.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::modules::tui::domain::view_state::{ProviderWizard, WIZARD_KINDS, WizardStep};
use crate::modules::tui::infrastructure::theme;
use crate::shared::kernel::provider::ProviderKind;

/// The rows the wizard occupies in `area`: a header, a blank, the body (one prompt + one row per kind on
/// the `Kind` step, or one prompt + one value row on a text step), a blank, and the hint — used by the
/// view to reserve exactly its height.
pub fn box_dims(area: Rect, wizard: &ProviderWizard) -> (u16, u16) {
    let body = match wizard.step {
        WizardStep::Kind => 1 + WIZARD_KINDS.len(),
        _ => 2,
    };
    // header + blank + body + blank + hint
    let height = ((3 + body + 1) as u16).min(area.height.max(1));
    (area.width, height)
}

pub fn render(wizard: &ProviderWizard, frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        format!(" novo provider · {}", step_label(wizard.step)),
        theme::dim(),
    ));
    lines.push(Line::default());

    if wizard.step == WizardStep::Kind {
        lines.push(Line::styled(
            " Escolha o tipo de provider:",
            theme::strong(),
        ));
        for (i, kind) in WIZARD_KINDS.iter().enumerate() {
            let (marker, style) = if i == wizard.kind_selected {
                ("❯ ", theme::accent())
            } else {
                ("  ", theme::dim())
            };
            lines.push(Line::from(vec![
                Span::styled(marker, style),
                Span::styled(kind_label(*kind), style),
            ]));
        }
    } else {
        lines.push(Line::styled(
            format!(" {}", prompt_for(wizard.step)),
            theme::strong(),
        ));
        lines.push(Line::from(vec![
            Span::styled(" › ", theme::accent()),
            Span::styled(field_display(wizard), theme::base()),
        ]));
    }

    lines.push(Line::default());
    lines.push(Line::styled(
        " Enter: próximo · Esc: cancelar",
        theme::dim(),
    ));

    frame.render_widget(Paragraph::new(lines).style(theme::base()), area);
}

fn step_label(step: WizardStep) -> &'static str {
    match step {
        WizardStep::Kind => "tipo (1/5)",
        WizardStep::BaseUrl => "endpoint (2/5)",
        WizardStep::Model => "modelo (3/5)",
        WizardStep::ExtraModels => "modelos extras (4/5)",
        WizardStep::ApiKey => "chave (5/5)",
    }
}

fn prompt_for(step: WizardStep) -> &'static str {
    match step {
        WizardStep::Kind => "",
        WizardStep::BaseUrl => "Base URL:",
        WizardStep::Model => "Modelo default:",
        WizardStep::ExtraModels => "Modelos extras (separados por vírgula, opcional):",
        WizardStep::ApiKey => "API key:",
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

/// The current field's value for display. The API key is masked to bullets so it never appears on
/// screen; an empty field renders blank.
fn field_display(wizard: &ProviderWizard) -> String {
    match wizard.step {
        WizardStep::BaseUrl => wizard.base_url.clone(),
        WizardStep::Model => wizard.model.clone(),
        WizardStep::ExtraModels => wizard.extra_models.clone(),
        WizardStep::ApiKey => "•".repeat(wizard.api_key.chars().count()),
        WizardStep::Kind => String::new(),
    }
}
