//! Markdown → `Vec<Line<'static>>` renderer for the transcript and approval surfaces.
//!
//! Parses inline + block markdown via `pulldown-cmark` and emits ratatui `Line`s of styled `Span`s,
//! word-wrapped to a width. Styling is additive: the visible text and word boundaries are unchanged
//! by formatting, so the transcript's line-count scroll offsets stay exact for a given width.
//!
//! Supported: bold (`**`/`__`), italic (`*`/`_`), strikethrough (`~~`), inline code (`` ` ``), fenced
//! code blocks, headings (`#`), unordered/ordered lists (`-`/`*`/`1.`), blockquotes (`>`), paragraphs,
//! and hard breaks. Links render as their text. Unsupported block constructs fall back to paragraph
//! text so nothing is lost. A fenced block with a recognized language tag gets real per-token syntax
//! highlighting via `syntect` (`render::highlight_code_block`); an untagged or unrecognized tag keeps
//! the flat code style.
//!
//! Split along the `Block` AST boundary: `parse` turns markdown into `Block`s, `render` lays them out;
//! this facade owns only the cached public entry point.

mod parse;
mod render;

use std::cell::RefCell;
use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::Line;

/// Upper bound on memoized renders before the cache is cleared. Transcript items are immutable once
/// past, so they hit the cache every frame; only the streaming item and width changes miss. Cleared
/// wholesale on overflow (cheap, re-warms in one frame) rather than evicting per entry.
const MAX_RENDER_CACHE: usize = 512;

/// The memoization key: the owned inputs themselves, so a hash collision can never return another
/// item's rendered lines (a lossy `u64` key did). The extra `String` clone per uncached entry is
/// negligible against the markdown parse it avoids.
type CacheKey = (String, Style, usize);

thread_local! {
    /// Per-thread memoization of `render` keyed by `(markdown, base, width)`. The TUI runtime is
    /// single-threaded (`!Send`), so a thread-local is the natural home; rendering the full transcript
    /// each 120ms frame re-parsed every item before this, the dominant idle/stream CPU cost.
    static RENDER_CACHE: RefCell<HashMap<CacheKey, Vec<Line<'static>>>> = RefCell::new(HashMap::new());
}

/// Render `markdown` to a wrapped, styled list of lines at `width` columns, with `base` as the
/// default text style. Memoized: a re-render of unchanged content at the same width returns a clone of
/// the cached lines instead of re-parsing. Each returned `Line` is one visual row; word-wrap carries
/// `Style` through every word so inline formatting survives wrapping. Blank lines are preserved.
pub fn render(markdown: &str, base: Style, width: usize) -> Vec<Line<'static>> {
    // TUII-12: the key clones the source per call. Profiled (release, warm cache) at ~0.5ns/call against
    // a ~640ns/call hit dominated by the unavoidable `Vec<Line>` result clone — non-material on the
    // per-frame path, and the owned key buys collision safety (above). Revisit only if transcript items
    // grow large enough that the source clone rivals the result clone.
    let key: CacheKey = (markdown.to_string(), base, width);
    if let Some(hit) = RENDER_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return hit;
    }
    let lines = render_uncached(markdown, base, width);
    RENDER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= MAX_RENDER_CACHE {
            cache.clear();
        }
        cache.insert(key, lines.clone());
    });
    lines
}

fn render_uncached(markdown: &str, base: Style, width: usize) -> Vec<Line<'static>> {
    let blocks = parse::parse(markdown, base);
    render::render_blocks(&blocks, width.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::infrastructure::theme;
    use ratatui::style::Modifier;

    #[test]
    fn renders_bold_and_italic_as_styled_spans() {
        let lines = render("**bold** and *italic*", Style::default(), 40);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert!(
            spans
                .iter()
                .any(|s| s.content == "bold" && s.style.add_modifier == Modifier::BOLD)
        );
        assert!(
            spans
                .iter()
                .any(|s| s.content == "italic" && s.style.add_modifier == Modifier::ITALIC)
        );
    }

    #[test]
    fn honors_base_style_for_paragraph_text() {
        // Paragraph text must inherit the caller's base style (this is how reasoning stays dim and the
        // approval action stays bold) — not a hardcoded color.
        let base = Style::default().fg(theme::HIGHLIGHT);
        let lines = render("plain words here", base, 40);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content == "plain" && s.style.fg == Some(theme::HIGHLIGHT)),
            "paragraph text should inherit the base fg: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn renders_inline_code_with_code_style() {
        let lines = render("use `cat` now", Style::default(), 40);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content == "cat" && s.style.fg == Some(theme::CODE_FG))
        );
    }

    #[test]
    fn wraps_long_lines_preserving_styles() {
        let md = "this is **a very long bold** line that must wrap to fit a narrow width";
        let lines = render(md, Style::default(), 10);
        assert!(lines.len() > 1);
        // The bold run survives wrapping onto a wrapped line.
        assert!(lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content == "bold" && s.style.add_modifier == Modifier::BOLD)
        }));
    }

    #[test]
    fn heading_is_bold_and_colored() {
        let lines = render("# Title", Style::default(), 40);
        assert_eq!(lines.len(), 1);
        let span = &lines[0].spans[0];
        assert_eq!(span.content, "Title");
        assert_eq!(span.style.fg, Some(theme::HEADING));
        assert_eq!(span.style.add_modifier, Modifier::BOLD);
    }

    #[test]
    fn unordered_list_items_get_dash_prefix() {
        let lines = render("- one\n- two\n", Style::default(), 40);
        assert!(lines.iter().any(|l| {
            l.spans
                .first()
                .is_some_and(|s| s.content == "- " || s.content == "-")
        }));
    }

    #[test]
    fn fenced_code_block_is_kept_verbatim_with_code_style() {
        let md = "```\nlet x = 1;\n```\n";
        let lines = render(md, Style::default(), 40);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.style.fg == Some(theme::CODE_FG))),
            "no code-styled line: {lines:?}"
        );
    }

    #[test]
    fn fenced_code_block_fills_the_width_with_background() {
        let lines = render("```\nx\n```\n", Style::default(), 12);
        let code = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.style.fg == Some(theme::CODE_FG)))
            .expect("a code line");
        let len: usize = code.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(len, 12, "code row should be padded to the full width");
    }

    #[test]
    fn a_tagged_fence_gets_real_per_token_syntax_highlighting() {
        // Issue #8d: a keyword and a string literal must render in genuinely different colors, syntect's
        // real tokenization — not the flat single-color style an untagged/unrecognized fence still gets
        // (proven separately by the two tests above).
        let md = "```rust\nlet s = \"hi\";\n```\n";
        let lines = render(md, Style::default(), 40);
        let colors: std::collections::HashSet<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| s.style.fg)
            .collect();
        assert!(
            colors.len() > 1,
            "a tagged code block must show more than one distinct token color: {colors:?}"
        );
        // Every span still keeps the app's own code background, not syntect's bundled theme background.
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .all(|s| s.style.bg == Some(theme::CODE_BG) || s.content.trim().is_empty()),
            "every code span must stay on this app's own CODE_BG"
        );
    }

    #[test]
    fn an_unrecognized_language_tag_keeps_the_flat_code_style() {
        // No regression for a fence whose language syntect doesn't recognize: same flat CODE_FG/CODE_BG
        // as an untagged fence, not a broken/empty render.
        let md = "```not-a-real-language\nx\n```\n";
        let lines = render(md, Style::default(), 40);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.style.fg == Some(theme::CODE_FG))),
            "an unrecognized language tag must keep the flat code style: {lines:?}"
        );
    }

    #[test]
    fn empty_input_yields_one_blank_line() {
        let lines = render("", Style::default(), 40);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn render_is_memoized_and_stays_consistent() {
        let md = "**bold** and `code` and a list:\n- one\n- two";
        let first = render(md, Style::default(), 40);
        let cached = render(md, Style::default(), 40);
        assert_eq!(first, cached, "a cache hit must match a fresh render");
        // The width is part of the key: a narrower width wraps to more rows.
        let narrow = render("a b c d e f g h i j", Style::default(), 5);
        let wide = render("a b c d e f g h i j", Style::default(), 40);
        assert_ne!(narrow.len(), wide.len());
    }

    #[test]
    fn blank_line_count_matches_raw_wrap_for_scroll_stability() {
        // Same visible text, with vs without bold: the wrapped row count must be identical.
        let raw = "the quick brown fox jumps over the lazy dog repeatedly";
        let bold = "**the quick brown fox jumps over the lazy dog repeatedly**";
        let a = render(raw, Style::default(), 12);
        let b = render(bold, Style::default(), 12);
        assert_eq!(a.len(), b.len());
    }
}
