use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::modules::tui::domain::model::{Model, Motion};
use crate::modules::tui::domain::transcript::{
    NoticeLevel, ToolActivity, ToolDiff, ToolStatus, TranscriptItem,
};
use crate::modules::tui::infrastructure::markdown;
use crate::modules::tui::infrastructure::text::greedy_wrap;
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::widgets::splash;

/// Max display width of the conversation column. Beyond this the body floats left against the void
/// (rather than stretching edge-to-edge on an ultrawide terminal), keeping comfortable line lengths.
const BODY_MAX_WIDTH: usize = 88;
/// How long a freshly-landed answer line takes to cool from forge-warm to polished steel.
const COOLING_MS: f32 = 150.0;

/// The cooling-reveal context for the active streaming answer: the motion preference, the current
/// frame's instant, and the landing instants of the completed lines. Borrowed for the render pass; all
/// derived colours are owned, so nothing leaks into the produced `'static` lines.
#[derive(Clone, Copy)]
struct Reveal<'a> {
    motion: Motion,
    now: Option<Instant>,
    landings: &'a [Instant],
}
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
pub fn render(model: &Model, frame: &mut Frame, area: Rect, motion: Motion) {
    if model.transcript.is_empty() {
        splash::render(
            frame,
            area,
            model.timeline.opened_at,
            model.timeline.render_at,
            motion,
        );
        return;
    }

    let width = (area.width as usize).clamp(1, BODY_MAX_WIDTH);
    let reveal = Reveal {
        motion,
        now: model.timeline.render_at,
        landings: &model.timeline.stream_landings,
    };
    let lines = build_transcript_lines(
        model.transcript.items(),
        width,
        model.expand_tools,
        model.status.streaming,
        model.has_modal(),
        reveal,
    );

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

/// Build the wrapped, styled transcript lines for the pane — the per-frame work, factored out of `render`
/// so it can be timed without a ratatui `Frame` (see the `render_cost` measurement). Pure in its inputs;
/// the markdown body of a finalized item is memoized by `markdown::render`, so a settled transcript pays
/// only this loop and the line clones each frame.
fn build_transcript_lines(
    items: &[TranscriptItem],
    width: usize,
    expanded: bool,
    streaming: bool,
    has_modal: bool,
    reveal: Reveal,
) -> Vec<Line<'static>> {
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
        let active = streaming && idx == last;
        render_item(item, width, expanded, active, reveal, &mut lines);
    }

    // Rack focus: while a confirmation is up, the whole transcript recedes one ramp step so the
    // borderless stanza pulls focus by depth, not by a drawn box. One static restyle — never animated,
    // so it stays a single diff and does not fight the markdown memoization.
    if has_modal {
        for line in &mut lines {
            recede(line);
        }
    }
    lines
}

fn render_item(
    item: &TranscriptItem,
    width: usize,
    expanded: bool,
    active: bool,
    reveal: Reveal,
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
            if active {
                // The signature: the answer materializes line by line, each settling from forge-warm
                // to polished steel as it lands, with a wet-ink caret on the line being written.
                render_streaming_answer(text, width, reveal, out);
            } else {
                render_body(text, Style::default().fg(theme::STEEL), width, false, out);
            }
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
        None => out.push(Line::from(vec![
            Span::styled("  ⎿ ", theme::dim()),
            Span::styled("● ", Style::default().fg(theme::WARNING)),
            Span::styled("…", theme::dim()),
        ])),
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
        Span::styled("  ⎿ ", theme::dim()),
        Span::styled("● ", Style::default().fg(marker_color)),
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

/// Render the actively streaming answer with the cooling-steel reveal: split into logical lines, colour
/// each completed line by its age along the cooling ramp (forge-warm → steel over ~150 ms), keep the
/// in-flight last line forge-warm, and mark the live writing edge with a `▌` caret. Only the newest one
/// or two lines have a non-settled age, so the per-frame diff is intrinsically tiny; once every line has
/// cooled the frame is byte-identical and the runtime relaxes to the idle cadence. Reduced motion freezes
/// the whole answer to steel with no caret — the layout is unchanged, just steady.
fn render_streaming_answer(text: &str, width: usize, reveal: Reveal, out: &mut Vec<Line<'static>>) {
    let logical: Vec<&str> = text.split('\n').collect();
    let last_idx = logical.len().saturating_sub(1);
    let caret = !reveal.motion.is_reduced();
    for (i, logical_line) in logical.iter().enumerate() {
        let in_flight = i == last_idx;
        let style = Style::default().fg(line_fg(i, in_flight, reveal));
        let rows = hard_wrap(logical_line, width);
        let last_row = rows.len().saturating_sub(1);
        for (r, row) in rows.into_iter().enumerate() {
            if in_flight && caret && r == last_row {
                out.push(Line::from(vec![
                    Span::styled(row, style),
                    Span::styled("▌", theme::accent()),
                ]));
            } else {
                out.push(Line::styled(row, style));
            }
        }
    }
}

/// The foreground of one streamed line: steel when motion is reduced; forge-warm while the line is still
/// being written (in-flight); otherwise the cooling ramp applied to its age. A completed line with no
/// recorded landing falls back to steel (already settled).
fn line_fg(index: usize, in_flight: bool, reveal: Reveal) -> Color {
    if reveal.motion.is_reduced() {
        return theme::STEEL;
    }
    if in_flight {
        return theme::COOLING_RAMP[0];
    }
    match (reveal.now, reveal.landings.get(index)) {
        (Some(now), Some(&landed)) => cooling_fg(now.saturating_duration_since(landed)),
        _ => theme::STEEL,
    }
}

/// Map a line's age to its cooling colour: forge-warm at age zero, polished steel at and beyond
/// `COOLING_MS`. Pure, so the reveal's colour curve is unit-testable like `spinner_frame`.
fn cooling_fg(age: Duration) -> Color {
    let t = (age.as_millis() as f32 / COOLING_MS).clamp(0.0, 1.0);
    theme::ramp(&theme::COOLING_RAMP, t)
}

/// Recede a line one ramp step: every span drops to the dim steel step so the transcript sinks behind a
/// confirmation. A single restyle of the foreground, leaving glyphs and layout untouched.
fn recede(line: &mut Line<'static>) {
    for span in &mut line.spans {
        span.style = span.style.fg(theme::STEEL_RAMP[3]);
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
        // Greedy word-wrap via the shared `greedy_wrap` primitive (display-width metric), so the
        // transcript and the editor wrap a line identically.
        rows.extend(greedy_wrap(logical, width));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooling_fg_starts_warm_and_settles_to_steel() {
        // Age zero is forge-warm; at and beyond the cooling window it is polished steel.
        assert_eq!(cooling_fg(Duration::ZERO), theme::CODE_FG);
        assert_eq!(cooling_fg(Duration::from_millis(150)), theme::STEEL);
        assert_eq!(cooling_fg(Duration::from_secs(1)), theme::STEEL);
    }

    #[test]
    fn line_fg_freezes_to_steel_under_reduced_motion() {
        let landings = [Instant::now()];
        let reduced = Reveal {
            motion: Motion::Reduced,
            now: Some(Instant::now()),
            landings: &landings,
        };
        assert_eq!(line_fg(0, false, reduced), theme::STEEL);
        assert_eq!(line_fg(0, true, reduced), theme::STEEL);
    }

    #[test]
    fn recede_drops_every_span_to_dim_steel() {
        let mut line = Line::from(vec![
            Span::styled("você", Style::default().fg(theme::HIGHLIGHT)),
            Span::styled(" oi", Style::default().fg(theme::STEEL)),
        ]);
        recede(&mut line);
        for span in &line.spans {
            assert_eq!(span.style.fg, Some(theme::STEEL_RAMP[3]));
        }
    }

    #[test]
    fn line_fg_in_flight_line_is_forge_warm() {
        let reveal = Reveal {
            motion: Motion::Full,
            now: None,
            landings: &[],
        };
        assert_eq!(line_fg(0, true, reveal), theme::CODE_FG);
    }

    use crate::modules::tui::domain::transcript::ToolActivity;

    /// A representative transcript: alternating user prompts, markdown assistant answers, and tool calls.
    fn sample_items(turns: usize) -> Vec<TranscriptItem> {
        let mut items = Vec::new();
        for i in 0..turns {
            items.push(TranscriptItem::User(format!(
                "how do I do thing number {i}?"
            )));
            items.push(TranscriptItem::Assistant(format!(
                "Here is **answer {i}** with a list:\n\n- first point about it\n- second point with `code`\n\nand a closing paragraph that wraps across the pane width to exercise the word-wrap path."
            )));
            items.push(TranscriptItem::Tool(ToolActivity {
                command: format!("rg 'thing {i}' ."),
                diff: None,
                result: Some((
                    ToolStatus::Ok,
                    "src/a.rs:1: match\nsrc/b.rs:2: match\nsrc/c.rs:3: match".to_string(),
                    Duration::from_millis(12),
                )),
            }));
        }
        items
    }

    /// Measurement for PERF-01 (not a correctness assertion): prints the per-frame cost of building the
    /// transcript lines for a settled (markdown-memoized) transcript, so the value of a line-level cache
    /// can be judged with data. Run on demand: `cargo test render_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement, not a correctness check; run with --ignored --nocapture"]
    fn render_cost() {
        let items = sample_items(40); // 120 items — a long session
        let reveal = Reveal {
            motion: Motion::Full,
            now: None,
            landings: &[],
        };
        // Warm the markdown memoization cache (the expensive parse happens once per unique body).
        let _ = build_transcript_lines(&items, 88, false, false, false, reveal);

        let iterations = 2000;
        let start = Instant::now();
        let mut sink = 0usize;
        for _ in 0..iterations {
            let lines = build_transcript_lines(&items, 88, false, false, false, reveal);
            sink = sink.wrapping_add(lines.len());
        }
        let per_frame = start.elapsed() / iterations;
        println!(
            "render_cost: {} items -> {} lines, {:?}/frame (markdown memoized), sink={sink}",
            items.len(),
            build_transcript_lines(&items, 88, false, false, false, reveal).len(),
            per_frame,
        );
    }

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
