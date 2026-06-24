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
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::view::view;
use crate::shared::kernel::error::AgentError;

/// The agent-turn future, boxed and `!Send`. Driven as a `select!` arm — never spawned — so no
/// `Send`/`'static` bound is needed and the engine borrows stay plain references.
type TurnFuture<'a> = Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentError>> + 'a>>;

const FRAME_INTERVAL: Duration = Duration::from_millis(120);

/// The full-screen TUI frontend: owns the engine handles and the UI model, runs the render/input loop,
/// and drives one agent turn at a time. The sole frontend, assembled in `app::wire`.
pub struct Tui {
    agent_loop: AgentLoop,
    sandbox: Sandbox,
    conversation: Conversation,
    model: Model,
    seed: Option<String>,
    /// Kept so `/new` can rebuild a fresh conversation with the same system prompt.
    system_prompt: &'static str,
}

impl Tui {
    pub fn new(
        agent_loop: AgentLoop,
        sandbox: Sandbox,
        system_prompt: &'static str,
        seed: Option<String>,
        model: String,
    ) -> Self {
        let workspace = sandbox.root().display().to_string();
        Self {
            agent_loop,
            sandbox,
            conversation: Conversation::new(system_prompt),
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
                        conversation = Conversation::new(system_prompt);
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
                            model.status.workspace = new_sandbox.root().display().to_string();
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
            match step {
                Step::Done(outcome) => done = Some(outcome),
                Step::Idle => {}
                Step::Apply(msg) => {
                    for effect in update(model, msg) {
                        match effect {
                            Effect::AnswerApproval(decision) => {
                                if let Some(reply) = pending_reply.take() {
                                    let _ = reply.send(decision);
                                }
                            }
                            Effect::CancelTurn => cancel.cancel(),
                            Effect::Quit => {
                                model.should_quit = true;
                                cancel.cancel();
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
                }
            }
            // Draw after applying the step, so the frame reflects the latest model state (no one-frame
            // lag between a streaming delta and the thinking line / transcript).
            model.status.elapsed_secs = started.elapsed().as_secs();
            terminal.draw(|frame| view(model, frame))?;
            if let Some(outcome) = done {
                break outcome;
            }
        }
    };

    // Drain any deltas/notices buffered when the turn future resolved, so nothing is lost.
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
            // A finished plan-mode turn offers its plan for approval before anything is executed.
            if model.approval_mode == ApprovalMode::Plan && !cancelled {
                model.pending_plan = Some(PendingPlan::default());
            }
        }
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
    let _ = update(model, Msg::TurnEnded);
}
