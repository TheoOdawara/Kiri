use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{
    NoticeLevel, ToolActivity, ToolDiff, ToolStatus, TranscriptItem,
};
use crate::modules::tui::infrastructure::markdown;
use crate::modules::tui::infrastructure::text::{chunk_by_width, display_width};
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::splash;

/// Max display width of the conversation column. Beyond this the body floats left against the void
/// (rather than stretching edge-to-edge on an ultrawide terminal), keeping comfortable line lengths.
const BODY_MAX_WIDTH: usize = 88;
/// Lines of tool output (or edit-diff old/new block) shown before eliding, unless expanded (Ctrl+O).
const PREVIEW_LINES: usize = 6;
/// Per side (old/new) diff lines shown before eliding, unless expanded.
const DIFF_LINES_PER_SIDE: usize = 6;
/// Columns each previewed body line is indented under its tool-call header.
const PREVIEW_INDENT_WIDTH: usize = 5;
/// Milliseconds in one second — the threshold below which an elapsed label stays in `ms`.
const MS_PER_SECOND: u128 = 1000;

/// Render the scrolling transcript. Items are pre-wrapped to the pane width (so scroll offsets are
/// exact line counts), then scrolled so the newest content is pinned to the bottom unless the user has
/// scrolled up (`scrollback`). Assistant and reasoning items are parsed as markdown so bold, italics,
/// code spans, lists, and headings render with styled spans instead of raw `**`/`*`. When the
/// conversation is empty, the brand splash takes the pane instead.
pub fn render(model: &Model, frame: &mut Frame, area: Rect) {
    if model.transcript.is_empty() {
        splash::render(frame, area);
        return;
    }

    let width = (area.width as usize).clamp(1, BODY_MAX_WIDTH);
    let items = model.transcript.items();
    let last = items.len().saturating_sub(1);
    let mut lines: Vec<Line> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        if !lines.is_empty() {
            // Two blank rows open a new turn (a user prompt); one separates items within a turn.
            lines.push(Line::default());
            if matches!(item, TranscriptItem::User(_)) {
                lines.push(Line::default());
            }
        }
        // The still-streaming item (the trailing assistant/reasoning while a turn streams) renders as
        // plain text — skipping the per-frame markdown parse keeps the ~30 fps stream cheap; once the
        // turn finishes it re-renders formatted (and is then memoized).
        let active = model.status.streaming && idx == last;
        render_item(item, width, model.expand_tools, active, &mut lines);
    }

    let total = lines.len() as u16;
    let max_offset = total.saturating_sub(area.height);
    let scrollback = model.scroll.scrollback.min(max_offset);
    let offset = max_offset - scrollback;

    frame.render_widget(
        Paragraph::new(lines)
            .scroll((offset, 0))
            .style(theme::base()),
        area,
    );
}

fn render_item(
    item: &TranscriptItem,
    width: usize,
    expanded: bool,
    active: bool,
    out: &mut Vec<Line<'static>>,
) {
    match item {
        TranscriptItem::User(text) => push_wrapped(
            &format!("você › {text}"),
            width,
            Style::default().fg(theme::HIGHLIGHT),
            out,
        ),
        TranscriptItem::Reasoning(text) => {
            // Thinking reads clearly apart from the answer: a label plus a dim, italic body.
            let style = theme::dim().add_modifier(Modifier::ITALIC);
            out.push(Line::styled("⋮ pensando", style));
            render_body(text, style, width, active, out);
        }
        TranscriptItem::Assistant(text) => {
            out.push(Line::styled("◆ kiri", Style::default().fg(theme::HEADING)));
            render_body(text, Style::default().fg(theme::STEEL), width, active, out);
        }
        TranscriptItem::Tool(activity) => render_tool(activity, width, expanded, out),
        TranscriptItem::Notice(level, text) => {
            let color = match level {
                NoticeLevel::Info => theme::WARNING,
                NoticeLevel::Error => theme::ERROR,
            };
            push_wrapped(text, width, Style::default().fg(color), out);
        }
    }
}

/// Render one tool call: a `⏺ command` line, an inline diff for edits, then the indented `⎿` result
/// (preview-bounded unless `expanded`). While the call is still running, a faint `⎿ …` placeholder
/// shows it is in flight.
fn render_tool(
    activity: &ToolActivity,
    width: usize,
    expanded: bool,
    out: &mut Vec<Line<'static>>,
) {
    let running = activity.result.is_none();
    let cmd_color = if running {
        theme::HIGHLIGHT
    } else {
        theme::STEEL
    };
    out.push(Line::from(vec![
        Span::styled("⏺ ", Style::default().fg(cmd_color)),
        Span::styled(activity.command.clone(), Style::default().fg(cmd_color)),
    ]));

    if let Some(diff) = &activity.diff {
        render_diff(diff, width, expanded, out);
    }

    match &activity.result {
        None => out.push(Line::styled("  ⎿ …", theme::dim())),
        Some((status, output, elapsed)) => render_result(
            *status,
            output,
            *elapsed,
            activity.diff.is_some(),
            width,
            expanded,
            out,
        ),
    }
}

/// Render the `⎿` result block: a leading marker line carrying the first output line and the elapsed
/// time, then (for multi-line read/list/search output) a bounded preview of the remaining lines with
/// an elision hint. Errors render red, declines dim. A diff already showed the change, so an edit's
/// one-line confirmation is enough.
fn render_result(
    status: ToolStatus,
    output: &str,
    elapsed: Duration,
    has_diff: bool,
    width: usize,
    expanded: bool,
    out: &mut Vec<Line<'static>>,
) {
    let (marker_color, text_style) = match status {
        ToolStatus::Ok => (theme::SUCCESS, theme::dim()),
        ToolStatus::Error => (theme::ERROR, Style::default().fg(theme::ERROR)),
        ToolStatus::Declined => (theme::BRAND, theme::dim()),
    };
    let detail = match status {
        ToolStatus::Declined => "recusado pelo usuário",
        _ => output.trim_end_matches('\n'),
    };
    let lines: Vec<&str> = if detail.is_empty() {
        vec!["(vazio)"]
    } else {
        detail.split('\n').collect()
    };

    let head = format!("{} · {}", lines[0], fmt_dur(elapsed));
    out.push(Line::from(vec![
        Span::styled("  ⎿ ", Style::default().fg(marker_color)),
        Span::styled(head, text_style),
    ]));

    // For read/list/search the remaining output lines are the value the user wants to see; preview
    // them (bounded) unless expanded. Edits already rendered a diff, so skip their body.
    if has_diff || lines.len() <= 1 {
        return;
    }
    let body = &lines[1..];
    let shown = if expanded {
        body.len()
    } else {
        body.len().min(PREVIEW_LINES)
    };
    let indent = " ".repeat(PREVIEW_INDENT_WIDTH);
    for line in &body[..shown] {
        for row in hard_wrap(line, width.saturating_sub(PREVIEW_INDENT_WIDTH).max(1)) {
            out.push(Line::styled(format!("{indent}{row}"), text_style));
        }
    }
    if body.len() > shown {
        out.push(Line::styled(
            format!("     … (+{}) · ^O para expandir", body.len() - shown),
            theme::dim(),
        ));
    }
}

/// Render an `edit_file` change as red `-` / green `+` lines, wrapped, bounded per side unless
/// expanded. This is a literal old→new replacement block, not a line-level diff algorithm.
fn render_diff(diff: &ToolDiff, width: usize, expanded: bool, out: &mut Vec<Line<'static>>) {
    emit_diff_block(&diff.old, '-', theme::ERROR, width, expanded, out);
    emit_diff_block(&diff.new, '+', theme::SUCCESS, width, expanded, out);
}

fn emit_diff_block(
    text: &str,
    sign: char,
    color: Color,
    width: usize,
    expanded: bool,
    out: &mut Vec<Line<'static>>,
) {
    let lines: Vec<&str> = text.split('\n').collect();
    let shown = if expanded {
        lines.len()
    } else {
        lines.len().min(DIFF_LINES_PER_SIDE)
    };
    let style = Style::default().fg(color);
    for line in &lines[..shown] {
        for row in hard_wrap(&format!("  {sign} {line}"), width) {
            out.push(Line::styled(row, style));
        }
    }
    if lines.len() > shown {
        out.push(Line::styled(
            format!("  … (+{})", lines.len() - shown),
            theme::dim(),
        ));
    }
}

/// A compact elapsed label for a single tool call: milliseconds under a second, else seconds.
fn fmt_dur(elapsed: Duration) -> String {
    let ms = elapsed.as_millis();
    if ms < MS_PER_SECOND {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

/// Render an assistant/reasoning body. While it is the actively streaming item, emit plain wrapped text
/// so the per-frame markdown parse is skipped (cheap at ~30 fps); a finalized item goes through the
/// memoized markdown renderer, so bold/lists/code render once the turn ends.
fn render_body(text: &str, style: Style, width: usize, active: bool, out: &mut Vec<Line<'static>>) {
    if active {
        push_wrapped(text, width, style, out);
    } else {
        let mut rendered = markdown::render(text, style, width);
        out.append(&mut rendered);
    }
}

fn push_wrapped(text: &str, width: usize, style: Style, out: &mut Vec<Line<'static>>) {
    for row in hard_wrap(text, width) {
        out.push(Line::styled(row, style));
    }
}

/// Wrap by word to the pane width, preserving blank lines. Words longer than the width are hard-cut
/// so they never overflow. Width is a char count (display-width approximation), consistent with the
/// rest of the renderer and good enough for the transcript's mostly-ASCII content.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for logical in text.split('\n') {
        if logical.is_empty() {
            rows.push(String::new());
            continue;
        }
        wrap_line(logical, width, &mut rows);
    }
    rows
}

/// Greedy word-wrap one logical line into `rows`, splitting on spaces. A word wider than `width` is
/// chunked by chars so it fits without overflow.
fn wrap_line(line: &str, width: usize, rows: &mut Vec<String>) {
    let mut current = String::new();
    for word in line.split(' ') {
        let word_cols = display_width(word);
        if current.is_empty() {
            if word_cols <= width {
                current.push_str(word);
            } else {
                // Word alone exceeds the width: chunk it by display cells.
                rows.extend(chunk_by_width(word, width));
            }
        } else if display_width(&current) + 1 + word_cols <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            rows.push(std::mem::take(&mut current));
            if word_cols <= width {
                current.push_str(word);
            } else {
                rows.extend(chunk_by_width(word, width));
            }
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
}

#[cfg(test)]
mod tests {
    use super::hard_wrap;

    #[test]
    fn wraps_long_lines_and_preserves_blank_lines() {
        assert_eq!(hard_wrap("abcdef", 3), vec!["abc", "def"]);
        assert_eq!(hard_wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn word_wrap_breaks_at_spaces_and_chunks_long_words() {
        assert_eq!(
            hard_wrap("the quick brown fox", 5),
            vec!["the", "quick", "brown", "fox"]
        );
        assert_eq!(
            hard_wrap("a verylongword here", 4),
            vec!["a", "very", "long", "word", "here"]
        );
    }
}
