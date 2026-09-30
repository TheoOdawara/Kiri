//! Render layer: a `Block` AST → wrapped, styled ratatui `Line`s.
//!
//! Styling is already baked into each `Span` by `super::parse`, so this layer only handles layout:
//! word-wrapping, code framing, and quote/list prefixes.

use std::sync::LazyLock;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use super::parse::Block;
use crate::modules::tui::infrastructure::text::{chunk_by_width, display_width};
use crate::modules::tui::infrastructure::theme;

/// Syntect's bundled syntax definitions, loaded once (issue #8d: real per-token syntax highlighting in
/// fenced code blocks, replacing the previous hand-rolled keyword-only tokenizer). `_newlines` variant
/// keeps line endings in the parsed text, matching how `HighlightLines` expects to be fed one line at a
/// time.
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// The bundled dark theme syntect highlights against. Only each token's foreground color is used (see
/// `to_ratatui_style`); the background stays `theme::CODE_BG` so a code block reads as part of this
/// app's own palette, not a mismatched embedded theme. Looked up by `.get` rather than indexing: `"5"` is
/// an unpinned minor/patch range, so a future semver-compatible syntect release renaming or dropping this
/// bundled theme must degrade to `Theme::default()` (an unstyled pass-through, not real highlighting)
/// rather than panic on the very first code-block render.
static CODE_THEME: LazyLock<Theme> = LazyLock::new(|| {
    ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap_or_default()
});

/// Above this many bytes, a code block skips syntect entirely and keeps the flat code style — the same
/// cap convention already used for model/tool-supplied content elsewhere (`exec::EXEC_MAX_BYTES`,
/// `support::READ_FILE_MAX_BYTES`). Syntect's regex-driven grammar matching runs synchronously on the
/// render thread with no cancellation; an unbounded or adversarially-crafted block (e.g. relayed
/// uncapped from an MCP tool result) could otherwise stall the whole TUI (security review of issue #8d).
const MAX_HIGHLIGHT_BYTES: usize = 64 * 1024;

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
            for mut line_spans in highlight_code_block(lines, lang.as_deref()) {
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

/// Highlight every line of one fenced code block, using one `HighlightLines` instance for the whole
/// block so multi-line constructs (a block comment, a triple-quoted string) parse correctly across line
/// boundaries — a per-line-fresh highlighter would restart mid-construct on every line.
///
/// `lang` unrecognized by syntect (including no tag at all — a bare fence), or a block over
/// `MAX_HIGHLIGHT_BYTES`, keeps the exact flat `CODE_FG`/`CODE_BG` style this rendered before issue #8d,
/// rather than force a plain-text pass through syntect that might carry a slightly different default
/// foreground from the bundled theme (or, for the size cap, risk a slow synchronous highlight on the
/// render thread).
fn highlight_code_block(lines: &[String], lang: Option<&str>) -> Vec<Vec<Span<'static>>> {
    let base_style = Style::default().fg(theme::CODE_FG).bg(theme::CODE_BG);
    let flat = || {
        lines
            .iter()
            .map(|line| vec![Span::styled(format!(" {line}"), base_style)])
            .collect()
    };
    let total_bytes: usize = lines.iter().map(String::len).sum();
    if total_bytes > MAX_HIGHLIGHT_BYTES {
        return flat();
    }
    // Known gap vs. the old hand-rolled tokenizer: syntect's bundled defaults recognize
    // rust/rs/python/py/go/javascript/js/json by token, but NOT typescript/ts/toml (verified directly
    // against `SyntaxSet::load_defaults_newlines`) — those now fall back to the flat style below instead
    // of the old fake-but-present keyword coloring. Locked by
    // `bundled_syntax_coverage_matches_the_known_gap` below, so a future syntect upgrade that changes
    // this set is caught rather than silently drifting further.
    let Some(syntax) = lang.and_then(|l| SYNTAX_SET.find_syntax_by_token(l)) else {
        return flat();
    };
    let mut highlighter = HighlightLines::new(syntax, &CODE_THEME);
    lines
        .iter()
        .map(|line| {
            let padded = format!(" {line}\n");
            match highlighter.highlight_line(&padded, &SYNTAX_SET) {
                Ok(ranges) => ranges
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(
                            text.trim_end_matches('\n').to_string(),
                            to_ratatui_style(style),
                        )
                    })
                    .collect(),
                // A syntax-definition error on this one line must never drop its text from the
                // transcript — fall back to the flat style for just this line, matching the
                // unrecognized-language path, rather than silently vanishing content.
                Err(_) => vec![Span::styled(format!(" {line}"), base_style)],
            }
        })
        .collect()
}

/// Convert a syntect per-token `Style` to a ratatui one. Only the foreground is taken from syntect — the
/// background stays `theme::CODE_BG` so every code block reads as part of this app's own dark palette,
/// not a differently-toned embedded theme. Known trade-off: `base16-ocean.dark` sets a distinct
/// background on a couple of scopes (notably `invalid.illegal`, e.g. an unterminated string), and that
/// "something's wrong here" visual cue is deliberately suppressed along with everything else's
/// background — accepted for consistency, not an oversight.
fn to_ratatui_style(style: syntect::highlighting::Style) -> Style {
    let fg = style.foreground;
    Style::default()
        .fg(Color::Rgb(fg.r, fg.g, fg.b))
        .bg(theme::CODE_BG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_syntax_coverage_matches_the_known_gap() {
        // Locks the exact language-token coverage `highlight_code_block`'s comment documents, so a
        // future syntect upgrade that silently changes this set is caught here instead of just quietly
        // degrading more (or fewer) language tags to the flat fallback.
        for recognized in [
            "rust",
            "rs",
            "python",
            "py",
            "go",
            "javascript",
            "js",
            "json",
        ] {
            assert!(
                SYNTAX_SET.find_syntax_by_token(recognized).is_some(),
                "{recognized} was expected to be recognized by the bundled syntax set"
            );
        }
        for unrecognized in ["typescript", "ts", "toml"] {
            assert!(
                SYNTAX_SET.find_syntax_by_token(unrecognized).is_none(),
                "{unrecognized} was expected to be the known, documented gap — if this now fails, \
                 syntect gained coverage: update the doc comment on highlight_code_block instead of \
                 just this test"
            );
        }
    }

    #[test]
    fn a_block_over_the_size_cap_skips_syntect_even_with_a_recognized_language() {
        // Security review of issue #8d: an oversized block (however it got that large — a verbose model
        // reply, or an uncapped upstream source) must never reach syntect's regex-driven grammar matching
        // on the render thread; it degrades to the flat style instead, same as an unrecognized language.
        let huge_line = "x".repeat(MAX_HIGHLIGHT_BYTES + 1);
        let lines = vec![huge_line];
        let spans = highlight_code_block(&lines, Some("rust"));
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].len(),
            1,
            "flat fallback is exactly one span per line"
        );
        assert_eq!(
            spans[0][0].style.fg,
            Some(theme::CODE_FG),
            "an oversized block must fall back to the flat code style, never syntect"
        );
    }

    #[test]
    fn a_recognized_language_under_the_cap_still_gets_real_highlighting() {
        // Companion to the size-cap test above: confirms the cap doesn't accidentally disable
        // highlighting for ordinary, well-under-the-limit code.
        let lines = vec!["let s = \"hi\";".to_string()];
        let spans = highlight_code_block(&lines, Some("rust"));
        let colors: std::collections::HashSet<_> =
            spans.iter().flatten().filter_map(|s| s.style.fg).collect();
        assert!(
            colors.len() > 1,
            "a small, recognized-language block must still get real per-token highlighting"
        );
    }
}
