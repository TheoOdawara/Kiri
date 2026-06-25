use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{EnableBracketedPaste, EnableMouseCapture, EventStream};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Interval};
use tokio_stream::StreamExt;

use crate::modules::agent::application::agent_loop::{AgentLoop, TurnOutcome};
use crate::modules::agent::application::approval_policy::{Approval, ApprovalMode};
use crate::modules::agent::domain::conversation::Conversation;
use crate::modules::agent::domain::message::Message;
use crate::modules::agent::domain::role::Role;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Msg, StreamKind};
use crate::modules::tui::application::update::update;
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::{NoticeLevel, Transcript, TranscriptItem};
use crate::modules::tui::domain::view_state::PendingPlan;
use crate::modules::tui::infrastructure::bridge::{Bridge, CancelToken, EngineMsg};
use crate::modules::tui::infrastructure::clipboard::{self, ClipboardContent};
use crate::modules::tui::infrastructure::input;
use crate::modules::tui::infrastructure::terminal_guard::TerminalGuard;
use crate::modules::tui::infrastructure::text;
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::view::view;
use crate::shared::kernel::error::AgentError;

/// The agent-turn future, boxed and `!Send`. Driven as a `select!` arm — never spawned — so no
/// `Send`/`'static` bound is needed and the engine borrows stay plain references.
type TurnFuture<'a> = Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentError>> + 'a>>;

const FRAME_INTERVAL: Duration = Duration::from_millis(120);

/// Minimum spacing between redraws while a turn streams. Finer than `FRAME_INTERVAL` so streamed text
/// flows at ~30 fps instead of appearing in coarse 120 ms blocks. It only paces draws that are already
/// being driven by incoming deltas, so an idle TUI still ticks at `FRAME_INTERVAL` and burns no extra CPU.
const STREAM_FRAME: Duration = Duration::from_millis(33);

/// The full-screen TUI frontend: owns the engine handles and the UI model, runs the render/input loop,
/// and drives one agent turn at a time. The sole frontend, assembled in `app::wire`.
pub struct Tui {
    agent_loop: AgentLoop,
    sandbox: Sandbox,
    conversation: Conversation,
    model: Model,
    seed: Option<String>,
    /// Kept so `/new` can rebuild a fresh conversation with the same system prompt. Owned because it
    /// may carry a per-session memory digest composed at wire time, not just the static base prompt.
    system_prompt: String,
}

impl Tui {
    pub fn new(
        agent_loop: AgentLoop,
        sandbox: Sandbox,
        system_prompt: String,
        seed: Option<String>,
        model: String,
    ) -> Self {
        let workspace = text::display_path(sandbox.root());
        Self {
            agent_loop,
            sandbox,
            conversation: Conversation::new(system_prompt.clone()),
            model: Model::new(model, workspace),
            seed,
            system_prompt,
        }
    }

    pub async fn run(self) -> Result<()> {
        let Tui {
            agent_loop,
            mut sandbox,
            mut conversation,
            mut model,
            seed,
            system_prompt,
        } = self;

        let mut terminal = ratatui::init();
        let _guard = TerminalGuard;
        // Best-effort: bracketed paste / mouse capture are nice-to-have enhancements; a terminal that
        // rejects them still runs fully. The TerminalGuard disables them symmetrically on exit.
        let _ = crossterm::execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture);

        // The editor widget owns its own styling; paint it with the brand theme once at startup.
        let cursor = ratatui::style::Style::default()
            .fg(theme::VOID)
            .bg(theme::HIGHLIGHT);
        let selection = ratatui::style::Style::default()
            .fg(theme::VOID)
            .bg(theme::BRAND);
        model.input.set_styles(theme::base(), cursor, selection);

        let (engine_tx, mut engine_rx) = mpsc::unbounded_channel::<EngineMsg>();
        let cancel = CancelToken::new();
        let mut bridge = Bridge::new(engine_tx, cancel.clone());
        let mut pending_reply: Option<oneshot::Sender<Approval>> = None;
        let mut events = EventStream::new();
        let mut ticker = time::interval(FRAME_INTERVAL);

        // An initial prompt from the CLI runs as the first turn.
        if let Some(line) = seed.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            match command::parse(&line) {
                Some(Command::Quit) => model.should_quit = true,
                // A non-quit command as the CLI seed is ignored; the seed is meant to be a prompt.
                Some(_) => {}
                None => {
                    model.history.record(&line);
                    model.transcript.push(TranscriptItem::User(line.clone()));
                    model.busy = true;
                    conversation.push(Message::user(line));
                    drive_turn(
                        &agent_loop,
                        &mut conversation,
                        &sandbox,
                        &mut bridge,
                        &mut model,
                        &mut engine_rx,
                        &cancel,
                        &mut pending_reply,
                        &mut terminal,
                        &mut events,
                        &mut ticker,
                    )
                    .await?;
                }
            }
        }

        while !model.should_quit {
            terminal.draw(|frame| view(&model, frame))?;

            // Resolve one input into a message, then handle it outside the select so the engine
            // handles are unambiguously free when a turn is armed.
            let msg = tokio::select! {
                biased;
                maybe = events.next() => match maybe {
                    Some(Ok(event)) => input::to_msg(event),
                    Some(Err(_)) => None,
                    None => {
                        model.should_quit = true;
                        None
                    }
                },
                _ = ticker.tick() => None,
            };
            let Some(msg) = msg else {
                continue;
            };

            for effect in update(&mut model, msg) {
                match effect {
                    Effect::SubmitPrompt { text, images } => {
                        let message = if images.is_empty() {
                            Message::user(text)
                        } else {
                            Message::user_multimodal(text, images)
                        };
                        conversation.push(message);
                        drive_turn(
                            &agent_loop,
                            &mut conversation,
                            &sandbox,
                            &mut bridge,
                            &mut model,
                            &mut engine_rx,
                            &cancel,
                            &mut pending_reply,
                            &mut terminal,
                            &mut events,
                            &mut ticker,
                        )
                        .await?;
                    }
                    Effect::CopyToClipboard(text) => clipboard::copy_text(&text),
                    Effect::PasteClipboard => paste_from_clipboard(&mut model),
                    Effect::Quit => model.should_quit = true,
                    Effect::NewSession => {
                        conversation = Conversation::new(system_prompt.clone());
                        model.transcript = Transcript::default();
                        model.attachments.clear();
                        model.scroll.pin();
                        model.transcript.push(TranscriptItem::Notice(
                            NoticeLevel::Info,
                            "nova sessão".to_string(),
                        ));
                    }
                    Effect::ChangeWorkspace(path) => match sandbox.relocated(&path) {
                        Ok(new_sandbox) => {
                            model.status.workspace = text::display_path(new_sandbox.root());
                            sandbox = new_sandbox;
                            model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Info,
                                format!("workspace: {}", model.status.workspace),
                            ));
                        }
                        Err(error) => model.transcript.push(TranscriptItem::Notice(
                            NoticeLevel::Error,
                            format!("erro: {error:#}"),
                        )),
                    },
                    Effect::ApprovePlan(mode) => {
                        model.approval_mode = mode;
                        let notice = if mode == ApprovalMode::Auto {
                            "▶ executando o plano (auto)"
                        } else {
                            "▶ executando o plano"
                        };
                        model.transcript.push(TranscriptItem::Notice(
                            NoticeLevel::Info,
                            notice.to_string(),
                        ));
                        model.busy = true;
                        conversation.push(Message::user(
                            "Plano aprovado. Prossiga com a execução.".to_string(),
                        ));
                        drive_turn(
                            &agent_loop,
                            &mut conversation,
                            &sandbox,
                            &mut bridge,
                            &mut model,
                            &mut engine_rx,
                            &cancel,
                            &mut pending_reply,
                            &mut terminal,
                            &mut events,
                            &mut ticker,
                        )
                        .await?;
                    }
                    Effect::AnswerApproval(_) | Effect::CancelTurn => {}
                }
            }
        }

        Ok(())
    }
}

/// Read the OS clipboard and route it into the buffer: an image becomes a staged attachment, text is
/// inserted at the cursor. Best-effort — an empty or unreadable clipboard is a no-op.
fn paste_from_clipboard(model: &mut Model) {
    // `update` for these messages produces no effects (they only mutate the model), so the returned
    // Vec is intentionally discarded — there is nothing for the runtime to perform.
    match clipboard::read() {
        ClipboardContent::Image(attachment) => {
            let _ = update(model, Msg::ImageAttached(attachment));
        }
        ClipboardContent::Text(text) => {
            let _ = update(model, Msg::Paste(text));
        }
        ClipboardContent::Empty => {}
    }
}

/// One step the turn loop's `select!` produced.
enum Step {
    Done(Result<TurnOutcome, AgentError>),
    Apply(Msg),
    Idle,
}

/// Whether applying `msg` must force an immediate redraw. Stream deltas and the periodic tick are
/// throttled to at most one draw per `FRAME_INTERVAL`, so a burst of tokens coalesces into a single
/// re-render; every structural change (tool lines, approvals, turn boundaries, user input) draws at
/// once for responsiveness.
fn forces_draw(msg: &Msg) -> bool {
    !matches!(msg, Msg::StreamDelta(..) | Msg::Tick)
}

/// The spinner frame index for an elapsed time: one step per `FRAME_INTERVAL`. Wrapping into the glyph
/// table is the renderer's job (`% SPINNER.len()`). Pure, so the animation cadence is unit-testable and
/// is driven by wall clock rather than message arrival.
fn spinner_frame(elapsed: Duration) -> usize {
    (elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize
}

/// Drive one agent turn to completion while keeping the UI live: stream deltas render, approvals show
/// a prompt, and ^C cancels cooperatively. The agent future borrows `conversation`/`sandbox`/`bridge`
/// only inside the inner block, so the caller may start another turn afterward.
#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    agent_loop: &AgentLoop,
    conversation: &mut Conversation,
    sandbox: &Sandbox,
    bridge: &mut Bridge,
    model: &mut Model,
    engine_rx: &mut mpsc::UnboundedReceiver<EngineMsg>,
    cancel: &CancelToken,
    pending_reply: &mut Option<oneshot::Sender<Approval>>,
    terminal: &mut DefaultTerminal,
    events: &mut EventStream,
    ticker: &mut Interval,
) -> Result<()> {
    cancel.reset();
    let started = Instant::now();
    // The approval mode is fixed for this turn; cycling it mid-turn applies to the next one.
    let mode = model.approval_mode;

    let result = {
        let mut turn: TurnFuture = Box::pin(agent_loop.run(conversation, sandbox, mode, bridge));
        let mut last_draw = Instant::now();
        loop {
            let step = tokio::select! {
                biased;
                maybe = events.next() => match maybe {
                    Some(Ok(event)) => input::to_msg(event).map(Step::Apply).unwrap_or(Step::Idle),
                    _ => Step::Idle,
                },
                Some(engine) = engine_rx.recv() => Step::Apply(engine_msg(engine, pending_reply)),
                _ = ticker.tick() => Step::Apply(Msg::Tick),
                outcome = &mut turn => Step::Done(outcome),
            };

            let mut done: Option<_> = None;
            // Forced steps redraw immediately; throttled ones (stream deltas, ticks) wait for the frame.
            let mut force = false;
            match step {
                Step::Done(outcome) => {
                    done = Some(outcome);
                    force = true;
                }
                Step::Idle => {}
                Step::Apply(msg) => {
                    force = forces_draw(&msg);
                    for effect in update(model, msg) {
                        match effect {
                            Effect::AnswerApproval(decision) => {
                                if let Some(reply) = pending_reply.take() {
                                    // Best-effort: the engine awaits this reply, but if the turn future
                                    // was already dropped (cancel/quit) the receiver is gone — a failed
                                    // send is then expected and harmless.
                                    let _ = reply.send(decision);
                                }
                            }
                            Effect::CancelTurn => {
                                cancel.cancel();
                                // Break the select! loop immediately — dropping the turn future
                                // kills any running child process (kill_on_drop on run_command).
                                done = Some(Ok(TurnOutcome::Aborted));
                                force = true;
                            }
                            Effect::Quit => {
                                model.should_quit = true;
                                cancel.cancel();
                                done = Some(Ok(TurnOutcome::Aborted));
                                force = true;
                            }
                            // Clipboard chords stay live during a turn (composing the next prompt).
                            Effect::CopyToClipboard(text) => clipboard::copy_text(&text),
                            Effect::PasteClipboard => paste_from_clipboard(model),
                            Effect::SubmitPrompt { .. }
                            | Effect::NewSession
                            | Effect::ChangeWorkspace(_)
                            | Effect::ApprovePlan(_) => {}
                        }
                    }
                    // Coalesce a burst: drain every engine message already queued before drawing, so
                    // many tokens that arrived together become one re-render instead of one per token.
                    // These messages only mutate the model (no effects); a structural one among them
                    // (tool line, approval, turn boundary) still forces an immediate draw.
                    while let Ok(engine) = engine_rx.try_recv() {
                        let queued = engine_msg(engine, pending_reply);
                        force |= forces_draw(&queued);
                        let _ = update(model, queued);
                    }
                }
            }
            model.status.elapsed_secs = started.elapsed().as_secs();
            // The spinner animates by wall clock, so its rate is independent of message cadence — it
            // spins during the wait for the first token and during tool execution, not only while
            // content streams.
            model.status.spinner_frame = spinner_frame(started.elapsed());
            // Draw on a forced step or once the stream-frame budget elapsed. Incoming deltas pace the
            // redraws at ~30 fps (smooth, no coarse blocks), while the 120ms ticker still guarantees a
            // periodic draw during a quiet wait. Coalescing keeps it to one transcript re-render per
            // frame rather than one per token (the cause of the lag).
            // A draw failure must NOT `?`-propagate out of this loop: that would skip `on_turn_end` and
            // leave `model.busy` stuck true, silently deadening every future submit. End the turn with
            // the error instead, so cleanup always runs.
            if force || last_draw.elapsed() >= STREAM_FRAME {
                if let Err(error) = terminal.draw(|frame| view(model, frame)) {
                    break Err(AgentError::Io(error));
                }
                last_draw = Instant::now();
            }
            if let Some(outcome) = done {
                break outcome;
            }
        }
    };

    // Drain any deltas/notices buffered when the turn future resolved, so nothing is lost. These
    // messages only mutate the model (no effects), so the returned Vec is intentionally discarded.
    while let Ok(engine) = engine_rx.try_recv() {
        let _ = update(model, engine_msg(engine, pending_reply));
    }

    let cancelled = cancel.is_cancelled();
    on_turn_end(result, cancelled, model, conversation);
    cancel.reset();
    *pending_reply = None;
    Ok(())
}

/// Translate an engine message into a UI message, capturing an approval's reply channel on the way.
fn engine_msg(engine: EngineMsg, pending_reply: &mut Option<oneshot::Sender<Approval>>) -> Msg {
    match engine {
        EngineMsg::Began => Msg::TurnBegan,
        EngineMsg::Reasoning(text) => Msg::StreamDelta(StreamKind::Reasoning, text),
        EngineMsg::Content(text) => Msg::StreamDelta(StreamKind::Content, text),
        EngineMsg::ToolStarted { command, diff } => Msg::ToolStarted { command, diff },
        EngineMsg::ToolFinished {
            status,
            output,
            elapsed,
        } => Msg::ToolFinished {
            status,
            output,
            elapsed,
        },
        EngineMsg::Finished => Msg::TurnFinished,
        EngineMsg::Approval { pending, reply } => {
            *pending_reply = Some(reply);
            Msg::ApprovalRequested(pending)
        }
    }
}

/// Apply the turn's outcome: surface errors, roll back the conversation, and reset per-turn UI state.
/// A user cancel (^C) is reported as such, not as an error.
fn on_turn_end(
    result: Result<TurnOutcome, AgentError>,
    cancelled: bool,
    model: &mut Model,
    conversation: &mut Conversation,
) {
    match result {
        Ok(TurnOutcome::Completed) => {
            if turn_produced_nothing(conversation) {
                // A 200 with an empty assistant reply and no tool activity: the provider returned
                // nothing usable (e.g. an empty stream). Surface it — never silent.
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Error,
                    "o provedor não retornou conteúdo — verifique o modelo/endpoint".to_string(),
                ));
            }
        }
        // A plan-mode turn called `present_plan`: render the finished plan and open the approval box.
        // The box appears ONLY here — never on a plain text turn — so the model may think or ask
        // questions in plan mode without prematurely triggering approval, and the plan shown is always
        // the complete tool argument, never a half-streamed transcript.
        Ok(TurnOutcome::PlanProposed(plan)) if !cancelled => {
            model.transcript.push(TranscriptItem::Assistant(plan));
            model.pending_plan = Some(PendingPlan::default());
        }
        Ok(TurnOutcome::PlanProposed(_)) => {}
        Ok(TurnOutcome::Aborted) => model.should_quit = true,
        Err(error) => {
            if cancelled {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Info,
                    "⨯ cancelado".to_string(),
                ));
            } else {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Error,
                    format!("erro: {error}"),
                ));
            }
            conversation.rollback_dangling_user();
            if !cancelled && matches!(error, AgentError::ProviderRejected { .. }) {
                conversation.rollback_last_assistant_turn();
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Info,
                    "turno anterior descartado (request rejeitado pelo provedor)".to_string(),
                ));
            }
        }
    }
    // `TurnEnded` only resets per-turn model state (no effects); the returned Vec is intentionally
    // discarded.
    let _ = update(model, Msg::TurnEnded);
}

/// True when the turn ended with an empty assistant reply and no tool activity — the provider returned
/// a 200 with nothing usable. The agent loop appends the final assistant text even when it is blank, so
/// the trailing message is the signal: an assistant message with blank content and no tool calls. A
/// turn that ran tools (trailing `Role::Tool`) or produced real text is not "nothing".
fn turn_produced_nothing(conversation: &Conversation) -> bool {
    match conversation.messages().last() {
        Some(last) => {
            last.role == Role::Assistant
                && last.tool_calls.is_empty()
                && last.content.as_deref().unwrap_or("").trim().is_empty()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{on_turn_end, turn_produced_nothing};
    use crate::modules::agent::application::agent_loop::TurnOutcome;
    use crate::modules::agent::application::approval_policy::ApprovalMode;
    use crate::modules::agent::domain::conversation::Conversation;
    use crate::modules::agent::domain::message::Message;
    use crate::modules::tui::domain::model::Model;
    use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};

    fn has_error_notice(model: &Model) -> bool {
        model
            .transcript
            .items()
            .iter()
            .any(|item| matches!(item, TranscriptItem::Notice(NoticeLevel::Error, _)))
    }

    #[test]
    fn empty_completion_surfaces_a_notice_and_no_plan_box() {
        // The exact regression: a plan-mode turn whose provider returned nothing must NOT show a plan
        // box, and must surface a visible error instead of failing silently.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.approval_mode = ApprovalMode::Plan;
        model.busy = true;
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("faça um plano"));
        conversation.push(Message::assistant_text("")); // the empty reply the loop appended

        on_turn_end(
            Ok(TurnOutcome::Completed),
            false,
            &mut model,
            &mut conversation,
        );

        assert!(
            model.pending_plan.is_none(),
            "an empty turn must not pop a phantom plan box"
        );
        assert!(
            has_error_notice(&model),
            "an empty turn must surface an error notice"
        );
    }

    #[test]
    fn present_plan_outcome_renders_the_plan_and_offers_the_box() {
        // A plan is surfaced ONLY via the explicit `present_plan` tool (TurnOutcome::PlanProposed):
        // the plan text is rendered and the approval box opens.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.approval_mode = ApprovalMode::Plan;
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("faça um plano"));

        on_turn_end(
            Ok(TurnOutcome::PlanProposed(
                "## Plano\n1. fazer X".to_string(),
            )),
            false,
            &mut model,
            &mut conversation,
        );

        assert!(
            model.pending_plan.is_some(),
            "a proposed plan must offer the plan box"
        );
        assert!(
            model.transcript.items().iter().any(|item| matches!(
                item,
                TranscriptItem::Assistant(text) if text.contains("Plano")
            )),
            "the proposed plan text must be rendered in the transcript"
        );
        assert!(!has_error_notice(&model), "a proposed plan is not an error");
    }

    #[test]
    fn plain_plan_mode_completion_does_not_pop_the_box() {
        // A plain text turn in plan mode (the model thought aloud or asked a question, but did NOT
        // call present_plan) must NOT open the approval box — the old eager heuristic was the bug.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.approval_mode = ApprovalMode::Plan;
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("faça um plano"));
        conversation.push(Message::assistant_text(
            "Preciso de mais detalhes: qual módulo?",
        ));

        on_turn_end(
            Ok(TurnOutcome::Completed),
            false,
            &mut model,
            &mut conversation,
        );

        assert!(
            model.pending_plan.is_none(),
            "a plain plan-mode turn must not pop the box without present_plan"
        );
        assert!(!has_error_notice(&model), "a real reply is not an error");
    }

    #[test]
    fn spinner_frame_advances_one_step_per_frame_interval() {
        use super::{FRAME_INTERVAL, spinner_frame};
        use std::time::Duration;
        assert_eq!(spinner_frame(Duration::ZERO), 0);
        assert_eq!(spinner_frame(FRAME_INTERVAL - Duration::from_millis(1)), 0);
        assert_eq!(spinner_frame(FRAME_INTERVAL), 1);
        assert_eq!(spinner_frame(FRAME_INTERVAL * 5), 5);
    }

    #[test]
    fn a_tool_only_turn_is_not_treated_as_empty() {
        // A turn that ended on a tool result (e.g. a declined checkpoint) produced activity — it is not
        // "nothing", so no spurious error notice.
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("read a.txt"));
        conversation.push(Message::tool_result("c1", "hello"));
        assert!(!turn_produced_nothing(&conversation));
    }
}
