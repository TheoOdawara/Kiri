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
        Block::Code { lines } => {
            let style = Style::default().fg(theme::CODE_FG).bg(theme::CODE_BG);
            for line in lines {
                // Pad each row to the full width so the code background renders as a solid band
                // instead of stopping at the end of the text.
                let mut content = format!(" {line}");
                let len = display_width(&content);
                if len < width {
                    content.push_str(&" ".repeat(width - len));
                }
                out.push(Line::from(vec![Span::styled(content, style)]));
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
