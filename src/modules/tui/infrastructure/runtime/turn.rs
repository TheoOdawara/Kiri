//! Driving one agent turn while keeping the UI live, plus the prompt-submit and plan-approval effect
//! handlers that arm a turn and flush the session afterward.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;

use super::bridge::EngineMsg;
use super::input;
use crate::modules::agent::application::agent_loop::TurnOutcome;
use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Msg, StreamKind};
use crate::modules::tui::application::update::update;
use crate::modules::tui::domain::modal::PendingPlan;
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::TranscriptItem;
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::conversation::Conversation;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::role::Role;

use super::render::{copy_to_clipboard, draw_and_copy, paste_from_clipboard, place_cursor};
use super::{EngineHandles, RunLoop, UiDriver};

/// The agent-turn future, boxed and `!Send`. Driven as a `select!` arm — never spawned — so no
/// `Send`/`'static` bound is needed and the engine borrows stay plain references.
type TurnFuture<'a> = Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentError>> + 'a>>;

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
pub(super) fn spinner_frame(elapsed: Duration) -> usize {
    (elapsed.as_millis() / super::FRAME_INTERVAL.as_millis()) as usize
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
pub(super) fn on_turn_end(
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
                model
                    .notify_error("o provedor não retornou conteúdo — verifique o modelo/endpoint");
            }
        }
        // A plan-mode turn called `present_plan`: render the finished plan and open the approval box.
        // The box appears ONLY here — never on a plain text turn — so the model may think or ask
        // questions in plan mode without prematurely triggering approval, and the plan shown is always
        // the complete tool argument, never a half-streamed transcript.
        Ok(TurnOutcome::PlanProposed(plan)) if !cancelled => {
            model
                .transcript
                .push(TranscriptItem::PlanProposed(plan.clone()));
            model.pending_plan = Some(PendingPlan {
                plan,
                selected: 0,
                scroll: 0,
            });
        }
        Ok(TurnOutcome::PlanProposed(_)) => {}
        // A ^C while busy cancels just this turn: `drive_turn` sets the cancel token and synthesizes
        // `Aborted`, so `cancelled` is true here — show it and drop the dangling user message, but keep
        // the session alive. Only a genuine input-stream end (`cancelled == false`, e.g. the approval
        // channel closed) quits.
        Ok(TurnOutcome::Aborted) if cancelled => {
            model.notify_info("⨯ cancelado");
            conversation.rollback_dangling_user();
        }
        Ok(TurnOutcome::Aborted) => model.should_quit = true,
        Err(error) => {
            if cancelled {
                model.notify_info("⨯ cancelado");
            } else {
                model.notify_error(format!("erro: {error}"));
            }
            conversation.rollback_dangling_user();
            if !cancelled && matches!(error, AgentError::ProviderRejected { .. }) {
                conversation.rollback_last_assistant_turn();
                model.notify_info("turno anterior descartado (request rejeitado pelo provedor)");
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
pub(super) fn turn_produced_nothing(conversation: &Conversation) -> bool {
    match conversation.messages().last() {
        Some(last) => {
            last.role == Role::Assistant
                && last.tool_calls.is_empty()
                && last.content.as_deref().unwrap_or("").trim().is_empty()
        }
        None => false,
    }
}

impl RunLoop {
    /// Submit a prompt: push it (with any pasted images) as a user message, drive the turn, and flush.
    pub(super) async fn submit_prompt(
        &mut self,
        text: String,
        images: Vec<String>,
        ui: &mut UiDriver<'_>,
        engine: &mut EngineHandles<'_>,
    ) -> Result<()> {
        let message = if images.is_empty() {
            Message::user(text)
        } else {
            Message::user_multimodal(text, images)
        };
        self.conversation.push(message);
        self.drive_turn(ui, engine).await?;
        self.flush_session().await;
        Ok(())
    }

    /// Approve the proposed plan: adopt the chosen mode, announce it, push the go-ahead message, and run
    /// the executing turn, then flush.
    pub(super) async fn approve_plan(
        &mut self,
        mode: ApprovalMode,
        ui: &mut UiDriver<'_>,
        engine: &mut EngineHandles<'_>,
    ) -> Result<()> {
        self.model.approval_mode = mode;
        let notice = if mode == ApprovalMode::Auto {
            "▶ executando o plano (auto)"
        } else {
            "▶ executando o plano"
        };
        self.model.notify_info(notice);
        self.model.busy = true;
        self.conversation.push(Message::user(
            "Plano aprovado. Prossiga com a execução.".to_string(),
        ));
        self.drive_turn(ui, engine).await?;
        self.flush_session().await;
        Ok(())
    }

    /// Drive one agent turn to completion while keeping the UI live: stream deltas render, approvals show
    /// a prompt, and ^C cancels cooperatively. The agent future borrows the conversation/sandbox/bridge
    /// only inside the inner block, so the caller may start another turn afterward.
    pub(super) async fn drive_turn(
        &mut self,
        ui: &mut UiDriver<'_>,
        engine: &mut EngineHandles<'_>,
    ) -> Result<()> {
        engine.cancel.reset();
        let started = Instant::now();
        // The approval mode is fixed for this turn; cycling it mid-turn applies to the next one.
        let mode = self.model.approval_mode;

        let result = {
            let mut turn: TurnFuture = Box::pin(self.agent_loop.run(
                &mut self.conversation,
                &self.sandbox,
                mode,
                engine.bridge,
            ));
            let mut last_draw = Instant::now();
            loop {
                let step = tokio::select! {
                    biased;
                    maybe = ui.events.next() => match maybe {
                        Some(Ok(event)) => {
                            self.model.timeline.last_event_at = Some(Instant::now());
                            input::to_msg(event).map(Step::Apply).unwrap_or(Step::Idle)
                        }
                        _ => Step::Idle,
                    },
                    Some(received) = engine.engine_rx.recv() => {
                        Step::Apply(engine_msg(received, engine.pending_reply))
                    }
                    _ = ui.ticker.tick() => Step::Apply(Msg::Tick),
                    outcome = &mut turn => Step::Done(outcome),
                };

                // Stamp the frame before applying the step, so line landings (in `update`) and the draw
                // that shows them share one instant — a freshly landed line starts at age zero.
                self.model.timeline.render_at = Some(Instant::now());

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
                        for effect in update(&mut self.model, msg) {
                            match effect {
                                Effect::AnswerApproval(decision) => {
                                    if let Some(reply) = engine.pending_reply.take() {
                                        // Best-effort: the engine awaits this reply, but if the turn
                                        // future was already dropped (cancel/quit) the receiver is gone —
                                        // a failed send is then expected and harmless.
                                        let _ = reply.send(decision);
                                    }
                                }
                                Effect::CancelTurn => {
                                    engine.cancel.cancel();
                                    // Break the select! loop immediately — dropping the turn future
                                    // kills any running child process (kill_on_drop on run_command).
                                    done = Some(Ok(TurnOutcome::Aborted));
                                    force = true;
                                }
                                Effect::Quit => {
                                    self.model.should_quit = true;
                                    engine.cancel.cancel();
                                    done = Some(Ok(TurnOutcome::Aborted));
                                    force = true;
                                }
                                // Clipboard chords stay live during a turn (composing the next prompt).
                                Effect::CopyToClipboard(text) => {
                                    copy_to_clipboard(&mut self.model, &text)
                                }
                                Effect::PasteClipboard => paste_from_clipboard(&mut self.model),
                                Effect::PlaceCursor { col, row } => {
                                    place_cursor(&mut self.model, ui.terminal, col, row)
                                }
                                // A picker/wizard cannot open mid-turn, so these never arrive here.
                                Effect::SubmitPrompt { .. }
                                | Effect::NewSession
                                | Effect::ResumeLast
                                | Effect::ListSessions
                                | Effect::OpenSession(_)
                                | Effect::SyncPush
                                | Effect::ChangeWorkspace(_)
                                | Effect::ApprovePlan(_)
                                | Effect::SetModel(_)
                                | Effect::SetEffort(_)
                                | Effect::SetProvider(_)
                                | Effect::SaveProvider { .. }
                                | Effect::DeleteProvider(_)
                                | Effect::OpenFile(_) => {}
                            }
                        }
                        // Coalesce a burst: drain every engine message already queued before drawing, so
                        // many tokens that arrived together become one re-render instead of one per token.
                        // These messages only mutate the model (no effects); a structural one among them
                        // (tool line, approval, turn boundary) still forces an immediate draw.
                        while let Ok(received) = engine.engine_rx.try_recv() {
                            let queued = engine_msg(received, engine.pending_reply);
                            force |= forces_draw(&queued);
                            let _ = update(&mut self.model, queued);
                        }
                    }
                }
                self.model.status.elapsed_secs = started.elapsed().as_secs();
                // The spinner animates by wall clock, so its rate is independent of message cadence — it
                // spins during the wait for the first token and during tool execution, not only while
                // content streams.
                self.model.status.spinner_frame = spinner_frame(started.elapsed());
                // Draw on a forced step or once the stream-frame budget elapsed. Incoming deltas pace the
                // redraws at ~30 fps (smooth, no coarse blocks), while the 120ms ticker still guarantees a
                // periodic draw during a quiet wait. Coalescing keeps it to one transcript re-render per
                // frame rather than one per token (the cause of the lag).
                // A draw failure must NOT `?`-propagate out of this loop: that would skip `on_turn_end`
                // and leave `model.busy` stuck true, silently deadening every future submit. End the turn
                // with the error instead, so cleanup always runs.
                if force || last_draw.elapsed() >= super::STREAM_FRAME {
                    // break-not-`?`: a draw failure must still run `on_turn_end` so `busy` resets.
                    if let Err(error) = draw_and_copy(ui.terminal, &mut self.model) {
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
        while let Ok(received) = engine.engine_rx.try_recv() {
            let _ = update(&mut self.model, engine_msg(received, engine.pending_reply));
        }

        let cancelled = engine.cancel.is_cancelled();
        on_turn_end(result, cancelled, &mut self.model, &mut self.conversation);
        engine.cancel.reset();
        *engine.pending_reply = None;
        Ok(())
    }
}
