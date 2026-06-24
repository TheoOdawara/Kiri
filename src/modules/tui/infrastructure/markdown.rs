//! Markdown → `Vec<Line<'static>>` renderer for the transcript and approval surfaces.
//!
//! Parses inline + block markdown via `pulldown-cmark` and emits ratatui `Line`s of styled `Span`s,
//! word-wrapped to a width. Styling is additive: the visible text and word boundaries are unchanged
//! by formatting, so the transcript's line-count scroll offsets stay exact for a given width.
//!
//! Supported: bold (`**`/`__`), italic (`*`/`_`), strikethrough (`~~`), inline code (`` ` ``), fenced
//! code blocks, headings (`#`), unordered/ordered lists (`-`/`*`/`1.`), blockquotes (`>`), paragraphs,
//! and hard breaks. Links render as their text. Unsupported block constructs fall back to paragraph
//! text so nothing is lost.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::modules::tui::infrastructure::theme;

/// Upper bound on memoized renders before the cache is cleared. Transcript items are immutable once
/// past, so they hit the cache every frame; only the streaming item and width changes miss. Cleared
/// wholesale on overflow (cheap, re-warms in one frame) rather than evicting per entry.
const MAX_RENDER_CACHE: usize = 512;

thread_local! {
    /// Per-thread memoization of `render` keyed by `(markdown, base, width)`. The TUI runtime is
    /// single-threaded (`!Send`), so a thread-local is the natural home; rendering the full transcript
    /// each 120ms frame re-parsed every item before this, the dominant idle/stream CPU cost.
    static RENDER_CACHE: RefCell<HashMap<u64, Vec<Line<'static>>>> = RefCell::new(HashMap::new());
}

fn cache_key(markdown: &str, base: Style, width: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    markdown.hash(&mut hasher);
    base.hash(&mut hasher);
    width.hash(&mut hasher);
    hasher.finish()
}

/// Render `markdown` to a wrapped, styled list of lines at `width` columns, with `base` as the
/// default text style. Memoized: a re-render of unchanged content at the same width returns a clone of
/// the cached lines instead of re-parsing. Each returned `Line` is one visual row; word-wrap carries
/// `Style` through every word so inline formatting survives wrapping. Blank lines are preserved.
pub fn render(markdown: &str, base: Style, width: usize) -> Vec<Line<'static>> {
    let key = cache_key(markdown, base, width);
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
    let width = width.max(1);
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION;
    let parser = Parser::new_ext(markdown, options);

    let mut blocks: Vec<Block> = Vec::new();
    let mut ctx = ParseCtx::new(base);
    for event in parser {
        match event {
            Event::Start(tag) => ctx.start(tag, &mut blocks),
            Event::End(end) => ctx.end(end, &mut blocks),
            Event::Text(s) => ctx.text(s.to_string()),
            Event::Code(s) => ctx.code(s.to_string()),
            Event::SoftBreak => ctx.soft_break(),
            Event::HardBreak => ctx.hard_break(),
            Event::InlineHtml(s) => ctx.text(s.to_string()),
            Event::Html(s) => ctx.text(s.to_string()),
            Event::FootnoteReference(s) => ctx.text(s.to_string()),
            Event::TaskListMarker(_) => {}
            Event::Rule => {}
            Event::InlineMath(s) | Event::DisplayMath(s) => ctx.text(s.to_string()),
        }
    }
    ctx.finish(&mut blocks);

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

/// A parsed block with its resolved style context.
#[derive(Debug, Clone)]
enum Block {
    /// A paragraph or heading text: a list of soft-break-separated lines, each a list of styled runs.
    Text { lines: Vec<Vec<Span<'static>>> },
    /// A fenced code block: raw lines, no inline parsing.
    Code { lines: Vec<String> },
    /// A blockquote: inner blocks, rendered with a `│ ` prefix.
    Quote { inner: Vec<Block> },
    /// An unordered list item: inner blocks, prefixed with `- `.
    Item { marker: String, inner: Vec<Block> },
}

/// Accumulator for the parser's current inline context.
#[derive(Debug, Default)]
struct InlineAccum {
    /// Current line's styled runs.
    runs: Vec<Span<'static>>,
    /// Completed lines (separated by soft/hard breaks) within the current block.
    lines: Vec<Vec<Span<'static>>>,
    /// Active style modifiers (bold/italic/strikethrough) applied to new runs.
    bold: bool,
    italic: bool,
    strike: bool,
}

impl InlineAccum {
    fn push_text(&mut self, text: String, base: Style) {
        let mut style = base;
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strike {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        // Split on spaces so the wrapper can rejoin them; keep spaces as separate styled runs to
        // preserve exact word boundaries. A run is either a word or a single space.
        for part in text.split(' ') {
            if part.is_empty() {
                self.runs.push(Span::styled(" ", style));
            } else {
                self.runs.push(Span::styled(part.to_string(), style));
            }
        }
    }

    fn push_code(&mut self, text: String) {
        let style = Style::default()
            .fg(theme::CODE_FG)
            .bg(theme::CODE_BG)
            .add_modifier(Modifier::BOLD);
        // Inline code is kept as a single run; the wrapper treats it as a word.
        self.runs.push(Span::styled(text, style));
    }

    fn soft_break(&mut self) {
        // A soft break ends the current line of runs.
        let line = std::mem::take(&mut self.runs);
        self.lines.push(line);
    }

    fn hard_break(&mut self) {
        let line = std::mem::take(&mut self.runs);
        self.lines.push(line);
    }

    fn finish_line(&mut self) {
        if !self.runs.is_empty() {
            let line = std::mem::take(&mut self.runs);
            self.lines.push(line);
        }
    }
}

struct ParseCtx {
    /// The caller's default text style; paragraph/list/quote text inherits it (headings override with
    /// the heading accent). This is what makes reasoning render dim, assistant steel, approval bold.
    base: Style,
    accum: InlineAccum,
    /// Stack of (tag, heading_level) for nested formatting.
    fmt_stack: Vec<FmtTag>,
    /// Completed text lines for the current text block (paragraph/heading).
    current_text: Vec<Vec<Span<'static>>>,
    /// Current code block lines.
    current_code: Vec<String>,
    /// Quote/item nesting: stack of block builders.
    block_stack: Vec<BlockBuilder>,
    /// Whether we are inside a code block.
    in_code: bool,
    /// Ordered list item counter per nesting level.
    list_counters: Vec<u64>,
}

#[derive(Debug, Clone, Copy)]
enum FmtTag {
    Strong,
    Emphasis,
    Strikethrough,
    Heading,
}

enum BlockBuilder {
    Quote(Vec<Block>),
    Item { marker: String, inner: Vec<Block> },
}

impl ParseCtx {
    fn new(base: Style) -> Self {
        Self {
            base,
            accum: InlineAccum::default(),
            fmt_stack: Vec::new(),
            current_text: Vec::new(),
            current_code: Vec::new(),
            block_stack: Vec::new(),
            in_code: false,
            list_counters: Vec::new(),
        }
    }

    fn start(&mut self, tag: Tag, blocks: &mut Vec<Block>) {
        match tag {
            Tag::Paragraph => {
                self.accum = InlineAccum::default();
                self.current_text.clear();
            }
            Tag::Heading { level: _, .. } => {
                self.accum = InlineAccum::default();
                self.current_text.clear();
                self.fmt_stack.push(FmtTag::Heading);
                self.sync_fmt();
            }
            Tag::Emphasis => {
                self.fmt_stack.push(FmtTag::Emphasis);
                self.sync_fmt();
            }
            Tag::Strong => {
                self.fmt_stack.push(FmtTag::Strong);
                self.sync_fmt();
            }
            Tag::Strikethrough => {
                self.fmt_stack.push(FmtTag::Strikethrough);
                self.sync_fmt();
            }
            Tag::CodeBlock(_) => {
                self.in_code = true;
                self.current_code.clear();
            }
            Tag::BlockQuote(_) => self.block_stack.push(BlockBuilder::Quote(Vec::new())),
            Tag::List(start) => {
                if let Some(start) = start {
                    self.list_counters.push(start);
                } else {
                    self.list_counters.push(0);
                }
            }
            Tag::Item => {
                let marker = match self.list_counters.last_mut() {
                    Some(0) => "- ".to_string(),
                    Some(n) => {
                        let m = format!("{}. ", *n);
                        *n += 1;
                        m
                    }
                    None => "- ".to_string(),
                };
                self.block_stack.push(BlockBuilder::Item {
                    marker,
                    inner: Vec::new(),
                });
            }
            Tag::Link { .. } | Tag::Image { .. } | Tag::FootnoteDefinition(_) => {}
            _ => {}
        }
        let _ = blocks;
    }

    /// Rebuild the accum's bold/italic/strike flags from the current format stack.
    fn sync_fmt(&mut self) {
        self.accum.bold = self.fmt_stack.iter().any(|t| matches!(t, FmtTag::Strong));
        self.accum.italic = self.fmt_stack.iter().any(|t| matches!(t, FmtTag::Emphasis));
        self.accum.strike = self
            .fmt_stack
            .iter()
            .any(|t| matches!(t, FmtTag::Strikethrough));
    }

    fn end(&mut self, end: TagEnd, blocks: &mut Vec<Block>) {
        match end {
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                self.accum.finish_line();
                let lines = std::mem::take(&mut self.accum.lines);
                if matches!(end, TagEnd::Heading(_)) {
                    self.fmt_stack.pop();
                    self.sync_fmt();
                }
                self.push_block(Block::Text { lines }, blocks);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.fmt_stack.pop();
                self.sync_fmt();
            }
            TagEnd::CodeBlock => {
                let lines = std::mem::take(&mut self.current_code);
                self.in_code = false;
                self.push_block(Block::Code { lines }, blocks);
            }
            TagEnd::BlockQuote(_) => {
                self.flush_text_block(blocks);
                if let Some(BlockBuilder::Quote(inner)) = self.block_stack.pop() {
                    self.push_block(Block::Quote { inner }, blocks);
                }
            }
            TagEnd::Item => {
                self.flush_text_block(blocks);
                if let Some(BlockBuilder::Item { marker, inner }) = self.block_stack.pop() {
                    self.push_block(Block::Item { marker, inner }, blocks);
                }
            }
            TagEnd::List(_) => {
                self.list_counters.pop();
            }
            _ => {}
        }
    }

    fn text(&mut self, s: String) {
        if self.in_code {
            for line in s.split('\n') {
                self.current_code.push(line.to_string());
            }
            return;
        }
        // Headings get the heading accent; all other text inherits the caller's base style.
        let style = if self.fmt_stack.iter().any(|t| matches!(t, FmtTag::Heading)) {
            Style::default()
                .fg(theme::HEADING)
                .add_modifier(Modifier::BOLD)
        } else {
            self.base
        };
        self.accum.push_text(s, style);
    }

    fn code(&mut self, s: String) {
        self.accum.push_code(s);
    }

    fn soft_break(&mut self) {
        if self.in_code {
            self.current_code.push(String::new());
        } else {
            self.accum.soft_break();
        }
    }

    fn hard_break(&mut self) {
        if self.in_code {
            self.current_code.push(String::new());
        } else {
            self.accum.hard_break();
        }
    }

    fn finish(&mut self, blocks: &mut Vec<Block>) {
        self.flush_text_block(blocks);
    }

    /// Flush any pending inline content as a `Block::Text` into the nearest enclosing block builder.
    /// Needed because list items and blockquotes may contain loose `Text` events without an explicit
    /// `Paragraph` wrapper (pulldown-cmark 0.13 omits the paragraph for simple items).
    fn flush_text_block(&mut self, blocks: &mut Vec<Block>) {
        self.accum.finish_line();
        if self.accum.lines.is_empty() {
            return;
        }
        let lines = std::mem::take(&mut self.accum.lines);
        self.push_block(Block::Text { lines }, blocks);
    }

    /// Push a block to the nearest enclosing block builder (quote/item) or the top-level list.
    fn push_block(&mut self, block: Block, blocks: &mut Vec<Block>) {
        match self.block_stack.last_mut() {
            Some(BlockBuilder::Quote(inner)) | Some(BlockBuilder::Item { inner, .. }) => {
                inner.push(block);
            }
            None => blocks.push(block),
        }
    }
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
                let len = content.chars().count();
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
                    width.saturating_sub(marker.chars().count()),
                    &mut inner_out,
                );
            }
            for (i, line) in inner_out.iter().enumerate() {
                if i == 0 {
                    out.push(prepend_prefix(marker, line.clone()));
                } else {
                    let pad = " ".repeat(marker.chars().count());
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
        let cols = content.chars().count();
        if current.is_empty() {
            if cols <= width {
                current.push(run.clone());
                current_cols = cols;
            } else {
                // Hard-cut a long word.
                let style = run.style;
                let mut chars = content.chars().peekable();
                while chars.peek().is_some() {
                    let chunk: String = chars.by_ref().take(width).collect();
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
                let mut chars = content.chars().peekable();
                while chars.peek().is_some() {
                    let chunk: String = chars.by_ref().take(width).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

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
