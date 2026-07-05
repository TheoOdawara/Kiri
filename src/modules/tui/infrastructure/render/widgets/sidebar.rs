use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::text::{Line, Span};
use ratatui::style::Style;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::TranscriptItem;
use crate::modules::tui::infrastructure::theme;

/// Render the sidebar panel containing project info, model configurations, and session stats.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme::STEEL_RAMP[4]))
        .style(theme::base());
    
    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();

    // --- Section: BRAND HEADER ---
    lines.push(Line::styled(" ⬢ kiri", theme::strong()));
    lines.push(Line::styled("  Engineering-Grade Code Harness", theme::dim()));
    lines.push(Line::styled(" ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄", theme::dim()));
    lines.push(Line::default());

    // --- Section: SYSTEM STATUS ---
    lines.push(Line::styled(" ◆ HARNESS STATUS", theme::strong()));
    lines.push(Line::from(vec![
        Span::styled("  Status: ", theme::dim()),
        if model.busy {
            Span::styled("EXECUTANDO", Style::default().fg(theme::HIGHLIGHT))
        } else {
            Span::styled("AGUARDANDO", Style::default().fg(theme::SUCCESS))
        },
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Modo:   ", theme::dim()),
        Span::styled(format!("{:?}", model.approval_mode).to_uppercase(), Style::default().fg(theme::WARNING)),
    ]));
    lines.push(Line::default());

    // --- Section: CONTEXT ---
    lines.push(Line::styled(" ◆ CONTEXTO", theme::strong()));
    lines.push(Line::from(vec![
        Span::styled("  Provider: ", theme::dim()),
        Span::styled(&model.status.provider, Style::default().fg(theme::STEEL_RAMP[1])),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Modelo:   ", theme::dim()),
        Span::styled(
            fit_text(&model.status.model, inner_area.width.saturating_sub(13) as usize),
            Style::default().fg(theme::STEEL_RAMP[1])
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Esforço:  ", theme::dim()),
        Span::styled(format!("{:?}", model.status.effort).to_uppercase(), Style::default().fg(theme::STEEL_RAMP[1])),
    ]));
    lines.push(Line::default());

    // --- Section: WORKSPACE ---
    lines.push(Line::styled(" ◆ WORKSPACE", theme::strong()));
    lines.push(Line::from(vec![
        Span::styled("  Diretório: ", theme::dim()),
        Span::styled(
            fit_text(&model.status.workspace, inner_area.width.saturating_sub(14) as usize),
            Style::default().fg(theme::STEEL_RAMP[2])
        ),
    ]));
    lines.push(Line::default());

    // --- Section: MODIFIED FILES ---
    lines.push(Line::styled(" ◆ ARQUIVOS MODIFICADOS", theme::strong()));
    let modified_files = collect_modified_files(model);
    if modified_files.is_empty() {
        lines.push(Line::styled("  (nenhum arquivo alterado)", theme::dim()));
    } else {
        for file in modified_files.iter().take(8) {
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(theme::SUCCESS)),
                Span::styled(fit_text(file, inner_area.width.saturating_sub(6) as usize), Style::default().fg(theme::STEEL)),
            ]));
        }
        if modified_files.len() > 8 {
            lines.push(Line::styled(format!("  ... e mais {}", modified_files.len() - 8), theme::dim()));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).style(theme::base()),
        inner_area,
    );
}

fn fit_text(text: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    if text.len() <= limit {
        text.to_string()
    } else {
        let keep = limit.saturating_sub(3);
        format!("...{}", &text[text.len().saturating_sub(keep)..])
    }
}

fn collect_modified_files(model: &Model) -> Vec<String> {
    let mut files = std::collections::BTreeSet::new();
    for item in model.transcript.items() {
        if let TranscriptItem::Tool(activity) = item {
            let cmd = &activity.command;
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if parts.len() >= 2 {
                let tool = parts[0];
                if tool == "edit_file" || tool == "write_file" || tool == "create_dir" || tool == "delete_file" {
                    let path = parts[1].trim_matches('"').trim_matches('\'');
                    files.insert(path.to_string());
                }
            }
        }
    }
    files.into_iter().collect()
}
