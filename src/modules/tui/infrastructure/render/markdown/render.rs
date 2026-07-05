//! Render layer: a `Block` AST → wrapped, styled ratatui `Line`s.
//!
//! Styling is already baked into each `Span` by `super::parse`, so this layer only handles layout:
//! word-wrapping, code framing, and quote/list prefixes.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::parse::Block;
use crate::modules::tui::infrastructure::text::{chunk_by_width, display_width};
use crate::modules::tui::infrastructure::theme;

/// Render parsed `blocks` to wrapped `Line`s at `width` columns. Blocks are separated by a blank line;
/// an empty result yields one blank line so the transcript never collapses to nothing.
pub(super) fn render_blocks(blocks: &[Block], width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            out.push(Line::default());
        }
        render_block(block, width, &mut out);
    }
    if out.is_empty() {
        out.push(Line::default());
    }
    out
}

/// Render a parsed block into wrapped `Line`s, appending to `out`. Styling is already baked into each
/// `Span` at parse time (from the caller's base), so this only handles layout: wrapping, code framing,
/// and quote/list prefixes.
fn render_block(block: &Block, width: usize, out: &mut Vec<Line<'static>>) {
    match block {
        Block::Text { lines } => {
            for logical in lines {
                if logical.is_empty() {
                    out.push(Line::default());
                    continue;
                }
                wrap_spans(logical, width, out);
            }
        }
        Block::Code { lang, lines } => {
            for line in lines {
                let mut line_spans = highlight_code_line(&format!(" {line}"), lang.as_deref());
                let line_len: usize = line_spans
                    .iter()
                    .map(|s| display_width(s.content.as_ref()))
                    .sum();
                if line_len < width {
                    let padding = " ".repeat(width - line_len);
                    line_spans.push(Span::styled(padding, Style::default().bg(theme::CODE_BG)));
                }
                out.push(Line::from(line_spans));
            }
        }
        Block::Quote { inner } => {
            let mut inner_out: Vec<Line<'static>> = Vec::new();
            for (i, b) in inner.iter().enumerate() {
                if i > 0 {
                    inner_out.push(Line::default());
                }
                render_block(b, width.saturating_sub(2), &mut inner_out);
            }
            for line in inner_out {
                let prefixed = prepend_prefix("│ ", line);
                out.push(prefixed);
            }
        }
        Block::Item { marker, inner } => {
            let mut inner_out: Vec<Line<'static>> = Vec::new();
            for (i, b) in inner.iter().enumerate() {
                if i > 0 {
                    inner_out.push(Line::default());
                }
                render_block(
                    b,
                    width.saturating_sub(display_width(marker)),
                    &mut inner_out,
                );
            }
            for (i, line) in inner_out.iter().enumerate() {
                if i == 0 {
                    out.push(prepend_prefix(marker, line.clone()));
                } else {
                    let pad = " ".repeat(display_width(marker));
                    out.push(prepend_prefix(&pad, line.clone()));
                }
            }
        }
    }
}

/// Word-wrap a list of styled runs to `width` columns, appending `Line`s to `out`. Words are runs
/// that are not single-space; spaces are preserved as their own runs so they rejoin naturally. A
/// word longer than `width` is hard-cut by chars.
fn wrap_spans(runs: &[Span<'static>], width: usize, out: &mut Vec<Line<'static>>) {
    let width = width.max(1);
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_cols: usize = 0;

    for run in runs {
        let content = run.content.as_ref();
        if content == " " {
            // A space run: add it if there's room and we have content; else wrap.
            if current_cols < width && !current.is_empty() {
                current.push(run.clone());
                current_cols += 1;
            } else if !current.is_empty() {
                out.push(Line::from(std::mem::take(&mut current)));
                current_cols = 0;
            }
            continue;
        }
        let cols = display_width(content);
        if current.is_empty() {
            if cols <= width {
                current.push(run.clone());
                current_cols = cols;
            } else {
                // Hard-cut a long word by display cells.
                let style = run.style;
                for chunk in chunk_by_width(content, width) {
                    out.push(Line::from(vec![Span::styled(chunk, style)]));
                }
            }
        } else if current_cols + 1 + cols <= width {
            current.push(Span::raw(" "));
            current.push(run.clone());
            current_cols += 1 + cols;
        } else {
            out.push(Line::from(std::mem::take(&mut current)));
            if cols <= width {
                current.push(run.clone());
                current_cols = cols;
            } else {
                let style = run.style;
                for chunk in chunk_by_width(content, width) {
                    out.push(Line::from(vec![Span::styled(chunk, style)]));
                }
                current_cols = 0;
            }
        }
    }
    if !current.is_empty() {
        out.push(Line::from(current));
    }
}

/// Prepend a string prefix to a `Line`, preserving the styles of the original spans.
fn prepend_prefix(prefix: &str, line: Line<'static>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw(prefix.to_string())];
    spans.extend(line.spans);
    Line::from(spans)
}

fn highlight_code_line(line: &str, lang: Option<&str>) -> Vec<Span<'static>> {
    let base_style = Style::default().fg(theme::CODE_FG).bg(theme::CODE_BG);

    let Some(l) = lang else {
        return vec![Span::styled(line.to_string(), base_style)];
    };

    let l_lower = l.to_lowercase();
    if !matches!(
        l_lower.as_str(),
        "rust"
            | "rs"
            | "python"
            | "py"
            | "go"
            | "javascript"
            | "js"
            | "typescript"
            | "ts"
            | "json"
            | "toml"
    ) {
        return vec![Span::styled(line.to_string(), base_style)];
    }

    let keywords = [
        "fn", "let", "mut", "match", "struct", "enum", "impl", "use", "pub", "return", "if",
        "else", "for", "in", "loop", "while", "const", "var", "function", "import", "export",
        "from", "class", "def", "import", "as", "package", "func", "type",
    ];

    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("#") {
        return vec![Span::styled(
            line.to_string(),
            Style::default().fg(theme::BRAND).bg(theme::CODE_BG),
        )];
    }

    let mut spans = Vec::new();
    let mut current_word = String::new();
    let mut in_string = false;
    let mut string_char = '"';

    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        if in_string {
            current_word.push(c);
            if c == string_char && chars.get(i.saturating_sub(1)) != Some(&'\\') {
                in_string = false;
                spans.push(Span::styled(
                    current_word.clone(),
                    Style::default().fg(theme::SUCCESS).bg(theme::CODE_BG),
                ));
                current_word.clear();
            }
            i += 1;
            continue;
        }

        if c == '/' && chars.get(i + 1) == Some(&'/') && !in_string {
            if !current_word.is_empty() {
                spans.push(Span::styled(current_word.clone(), base_style));
                current_word.clear();
            }
            let comment: String = chars[i..].iter().collect();
            spans.push(Span::styled(
                comment,
                Style::default().fg(theme::BRAND).bg(theme::CODE_BG),
            ));
            break;
        }

        if c == '#' && !in_string && (l_lower == "python" || l_lower == "py" || l_lower == "toml") {
            if !current_word.is_empty() {
                spans.push(Span::styled(current_word.clone(), base_style));
                current_word.clear();
            }
            let comment: String = chars[i..].iter().collect();
            spans.push(Span::styled(
                comment,
                Style::default().fg(theme::BRAND).bg(theme::CODE_BG),
            ));
            break;
        }

        if (c == '"' || c == '\'') && !in_string {
            if !current_word.is_empty() {
                spans.push(Span::styled(current_word.clone(), base_style));
                current_word.clear();
            }
            in_string = true;
            string_char = c;
            current_word.push(c);
            i += 1;
            continue;
        }

        if c.is_alphanumeric() || c == '_' {
            current_word.push(c);
        } else {
            if !current_word.is_empty() {
                if keywords.contains(&current_word.as_str()) {
                    spans.push(Span::styled(
                        current_word.clone(),
                        Style::default().fg(theme::HEADING).bg(theme::CODE_BG),
                    ));
                } else if current_word.chars().next().unwrap().is_numeric() {
                    spans.push(Span::styled(
                        current_word.clone(),
                        Style::default().fg(theme::HIGHLIGHT).bg(theme::CODE_BG),
                    ));
                } else {
                    spans.push(Span::styled(current_word.clone(), base_style));
                }
                current_word.clear();
            }
            spans.push(Span::styled(c.to_string(), base_style));
        }
        i += 1;
    }

    if !current_word.is_empty() {
        if keywords.contains(&current_word.as_str()) {
            spans.push(Span::styled(
                current_word,
                Style::default().fg(theme::HEADING).bg(theme::CODE_BG),
            ));
        } else {
            spans.push(Span::styled(current_word, base_style));
        }
    }

    spans
}
