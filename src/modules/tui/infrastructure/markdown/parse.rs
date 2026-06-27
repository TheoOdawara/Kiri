//! Parse layer: markdown text → a `Block` AST, via `pulldown-cmark`'s event stream.
//!
//! Inline styling (bold/italic/strike/code/heading accent) is baked into each `Span` here, from the
//! caller's `base` style; the render layer only lays the resulting blocks out. The seam is the `Block`
//! type, shared with `super::render`.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::modules::tui::infrastructure::theme;

/// Parse `markdown` into a `Block` AST with `base` as the default text style. Runs the
/// `pulldown-cmark` event loop; the render layer turns the blocks into wrapped lines.
pub(super) fn parse(markdown: &str, base: Style) -> Vec<Block> {
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
    blocks
}

/// A parsed block with its resolved style context.
#[derive(Debug, Clone)]
pub(super) enum Block {
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
